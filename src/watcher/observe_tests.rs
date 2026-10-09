//! The observation pass over fixture transcripts: what it writes, and that
//! it writes it once whatever fails or restarts. Every name is invented.
use super::*;
use crate::projects::segments::Segment;
use crate::storage::Storage;
use chrono::DateTime;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;

/// A mark placed in the fixture's folders, paths and branches.
const SENTINEL: &str = "zq-sentinel";
const IDLE: u64 = 1800;

struct Fixture {
    dir: PathBuf,
    storage: Arc<Storage>,
    map: ProjectMap,
    a: i64,
    b: i64,
}

impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("mn-observe-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
        let a = storage.project_upsert("demoapp", "demoapp", None).unwrap();
        let b = storage.project_upsert("notes", "notes", None).unwrap();
        let mut map = ProjectMap::new(Path::new("/h/zq-sentinel-workspace/projects"));
        map.insert("demoapp", a);
        map.insert("notes", b);
        Self {
            dir,
            storage,
            map,
            a,
            b,
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn live(&self, format: Format) -> Observation {
        Observation::new(self.storage.clone(), format, IDLE)
    }

    /// Run a live pass of a new process over `files`.
    fn tick(&self, format: Format, files: &[PathBuf]) -> usize {
        self.live(format).tick(files, &self.map, false).unwrap()
    }

    /// Every row of a table as text, in the order of its first columns.
    fn dump(&self, table: &str) -> Vec<String> {
        let conn = self.storage.conn.lock().unwrap();
        crate::test_support::rows(&conn, &format!("SELECT * FROM {table} ORDER BY 1, 2"))
    }

    /// What the pass keeps of the segments, without the ids of rows: the
    /// project, the times, the source, the driver, and the activity.
    fn kept(&self) -> Vec<String> {
        let conn = self.storage.conn.lock().unwrap();
        crate::test_support::rows(
            &conn,
            "SELECT s.stream, s.driver, s.agent, s.project_id, s.started_at, s.last_at,
                    COALESCE(s.last_touch_at, '-'), s.source,
                    (SELECT COALESCE(group_concat(minute || ':' || records || ':' || touches, ' '), '')
                       FROM (SELECT * FROM segment_activity a
                              WHERE a.segment_id = s.id ORDER BY minute))
               FROM work_segments s ORDER BY s.stream, s.started_at, s.id",
        )
    }

    fn prompts(&self) -> Vec<String> {
        self.dump("owner_prompts")
    }

    fn checkpoint(&self, format: Format, path: &Path) -> Option<Checkpoint> {
        self.storage
            .observation_checkpoint(&stream_of(format.namespace, path))
            .unwrap()
    }
}

impl Drop for Fixture {
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

const IN_A: &str = "/h/zq-sentinel-workspace/projects/demoapp";
const IN_B: &str = "/h/zq-sentinel-workspace/projects/notes";

fn at(minute: u32, second: u32) -> String {
    format!("2026-09-30T10:{minute:02}:{second:02}Z")
}

/// A Claude Code record at `minute`, in the folder `cwd`.
fn claude(minute: u32, cwd: &str, role: &str, content: serde_json::Value) -> String {
    json!({"type": role, "uuid": format!("u{minute}"), "sessionId": "s", "cwd": cwd,
        "gitBranch": format!("{SENTINEL}-branch"), "entrypoint": "cli",
        "timestamp": at(minute, 0), "message": {"content": content}})
    .to_string()
}

fn prompt(minute: u32, cwd: &str) -> String {
    claude(minute, cwd, "user", json!("please look at the change"))
}

fn said(minute: u32, cwd: &str) -> String {
    claude(
        minute,
        cwd,
        "assistant",
        json!([{"type": "text", "text": "looking"}]),
    )
}

fn tool(minute: u32, cwd: &str, name: &str, project: &str) -> String {
    let input = json!({"file_path": format!("/h/zq-sentinel-workspace/projects/{project}/a.md")});
    claude(
        minute,
        cwd,
        "assistant",
        json!([{"type": "tool_use", "name": name, "input": input}]),
    )
}

fn closed(minute: u32, cwd: &str) -> String {
    let mut record: serde_json::Value = serde_json::from_str(&said(minute, cwd)).unwrap();
    record["message"]["stop_reason"] = "end_turn".into();
    record.to_string()
}

/// A session that begins in one project and writes into another.
fn session() -> Vec<String> {
    vec![
        prompt(0, IN_A),
        said(0, IN_A),
        tool(5, IN_A, "Edit", "notes"),
        said(6, IN_A),
        closed(15, IN_A),
    ]
}

#[test]
fn observation_the_pass_writes_the_segments_the_rules_give_and_their_activity() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &session());
    assert_eq!(f.tick(CLAUDE, std::slice::from_ref(&path)), 1);

    // The same records through the pure walk.
    let mut walker = Walker::new(IDLE);
    for line in session() {
        walker.record(
            &crate::projects::facts::claude(&line, &f.map).unwrap(),
            None,
        );
    }
    let (expected, _): (Vec<Segment>, _) = walker.finish();
    let kept = f.kept();
    assert_eq!(kept.len(), expected.len());
    assert_eq!(expected.len(), 2);
    let began = chrono::DateTime::parse_from_rfc3339(&at(0, 0))
        .unwrap()
        .timestamp()
        / 60;
    let minute = |m: i64| began + m;
    assert_eq!(expected[0].activity, [(minute(0), 2, 2)]);
    assert_eq!(
        expected[1].activity,
        [(minute(5), 1, 1), (minute(6), 1, 0), (minute(15), 1, 0)]
    );
    assert!(kept[0].contains(&format!(
        "|{}|2026-09-30 10:00:00|2026-09-30 10:05:00|",
        f.a
    )));
    assert!(kept[0].contains("|launch|"));
    assert!(kept[0].contains(&format!("{}:2:2", minute(0))));
    assert!(kept[1].contains(&format!(
        "|{}|2026-09-30 10:05:00|2026-09-30 10:15:00|",
        f.b
    )));
    // The write that named the project is its last touch.
    assert!(
        kept[1].contains("|2026-09-30 10:05:00|path|"),
        "{}",
        kept[1]
    );
    assert!(kept[1].contains(&format!(
        "{}:1:1 {}:1:0 {}:1:0",
        minute(5),
        minute(6),
        minute(15)
    )));
    assert!(kept.iter().all(|row| row.contains("|owner|claude|")));

    assert_eq!(f.prompts().len(), 1);
    assert!(f.prompts()[0].contains("2026-09-30 10:00:00"));
    let checkpoint = f.checkpoint(CLAUDE, &path).unwrap();
    assert_eq!(checkpoint.state.project, Some(f.b));
    assert_eq!(checkpoint.state.folder, Some(f.a));
    assert_eq!(checkpoint.last_kind, Kind::TurnEnd);
    assert_eq!(checkpoint.program, Some(false));
}

#[test]
fn observation_two_histories_with_the_same_segments_differ_in_their_activity() {
    let rows = |records: Vec<String>| {
        let f = Fixture::new();
        let path = f.file("session.jsonl");
        append(&path, &records);
        f.tick(CLAUDE, &[path]);
        (
            f.dump("work_segments").len(),
            f.kept(),
            f.dump("segment_activity"),
        )
    };
    let busy = rows(vec![
        prompt(0, IN_A),
        said(1, IN_A),
        said(2, IN_A),
        said(9, IN_A),
    ]);
    let still = rows(vec![
        prompt(0, IN_A),
        said(8, IN_A),
        said(8, IN_A),
        said(9, IN_A),
    ]);
    assert_eq!((busy.0, still.0), (1, 1));
    let times = |kept: &[String]| kept[0].split('|').take(7).collect::<Vec<_>>().join("|");
    assert_eq!(times(&busy.1), times(&still.1), "the segments are the same");
    assert_ne!(busy.2, still.2, "their minutes are not");
}

/// The rows a single uninterrupted pass over `records` leaves.
fn control(format: Format, name: &str, records: &[String]) -> (Vec<String>, Vec<String>) {
    let f = Fixture::new();
    let path = f.file(name);
    append(&path, records);
    f.tick(format, &[path]);
    (f.kept(), f.prompts())
}

#[test]
fn observation_a_failed_write_leaves_the_checkpoint_and_the_next_tick_writes_the_same_rows() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &session());
    let block = |on: bool| {
        let sql = if on {
            "CREATE TRIGGER test_block_segments BEFORE INSERT ON work_segments
             BEGIN SELECT RAISE(ABORT, 'injected store failure'); END;"
        } else {
            "DROP TRIGGER test_block_segments"
        };
        f.storage.conn.lock().unwrap().execute_batch(sql).unwrap();
    };
    block(true);
    let pass = f.live(CLAUDE);
    assert_eq!(
        pass.tick(std::slice::from_ref(&path), &f.map, false)
            .unwrap(),
        0
    );
    assert!(f.checkpoint(CLAUDE, &path).is_none());
    assert!(f.kept().is_empty() && f.prompts().is_empty());
    block(false);
    // The file did not change; the failed transcript is read again all the same.
    assert_eq!(
        pass.tick(std::slice::from_ref(&path), &f.map, false)
            .unwrap(),
        1
    );
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &session())
    );
}

#[test]
fn observation_two_writers_on_one_transcript_write_each_record_once() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &session()[..2]);
    f.tick(CLAUDE, std::slice::from_ref(&path));
    append(&path, &session()[2..]);

    // The live pass has read the new records when a backfill takes the
    // same transcript and writes first.
    let live = f.live(CLAUDE);
    let (storage, map) = (f.storage.clone(), f.map.clone());
    let mut ran = false;
    *live.before_write.lock().unwrap() = Some(Box::new(move |path: &Path| {
        if !std::mem::replace(&mut ran, true) {
            let other = Observation::new(storage.clone(), CLAUDE, IDLE).with_history();
            assert_eq!(other.tick(&[path.to_path_buf()], &map, true).unwrap(), 1);
        }
    }));
    live.tick(std::slice::from_ref(&path), &f.map, false)
        .unwrap();
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &session())
    );
}

#[test]
fn observation_a_file_rewritten_under_the_pass_is_read_again_from_the_start() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &session());
    let other = vec![prompt(20, IN_B), said(21, IN_B), said(30, IN_B)];

    let live = f.live(CLAUDE);
    let rewritten = other.clone();
    let mut ran = false;
    *live.before_write.lock().unwrap() = Some(Box::new(move |path: &Path| {
        if !std::mem::replace(&mut ran, true) {
            std::fs::write(path, "").unwrap();
            append(path, &rewritten);
            append(path, &rewritten[..1]);
        }
    }));
    // What was read is not what the file holds any more: nothing is kept.
    assert_eq!(
        live.tick(std::slice::from_ref(&path), &f.map, false)
            .unwrap(),
        0
    );
    assert!(f.kept().is_empty());
    assert!(f.checkpoint(CLAUDE, &path).is_none());
    // The next tick reads the file as it is now, from its start.
    assert_eq!(
        live.tick(std::slice::from_ref(&path), &f.map, false)
            .unwrap(),
        1
    );
    let mut expected = other.clone();
    expected.push(other[0].clone());
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &expected)
    );
}

#[test]
fn observation_a_replaced_transcript_starts_empty_and_replaces_its_segments() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &session());
    f.tick(CLAUDE, std::slice::from_ref(&path));
    assert_eq!(f.kept().len(), 2);

    // The same name, another session: told by the fingerprint of its head.
    let other = vec![prompt(40, IN_B), said(41, IN_B), closed(45, IN_B)];
    std::fs::write(&path, "").unwrap();
    append(&path, &other);
    assert_eq!(f.tick(CLAUDE, std::slice::from_ref(&path)), 1);
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &other)
    );
    let checkpoint = f.checkpoint(CLAUDE, &path).unwrap();
    assert_eq!(checkpoint.cursor.generation, 1);
    assert_eq!(checkpoint.state.project, Some(f.b));
    assert_eq!(
        f.dump("segment_activity").len(),
        3,
        "the minutes of the old segments went with them"
    );
}

#[test]
fn observation_a_restart_between_two_reads_switches_on_the_second() {
    let records = vec![
        prompt(0, IN_A),
        tool(1, IN_A, "Read", "notes"),
        tool(2, IN_A, "Read", "notes"),
        said(9, IN_A),
    ];
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &records[..2]);
    f.tick(CLAUDE, std::slice::from_ref(&path));
    let between = f.checkpoint(CLAUDE, &path).unwrap();
    assert_eq!(
        (between.state.project, between.state.candidate),
        (Some(f.a), Some(f.b))
    );
    // A new process reads the rest.
    append(&path, &records[2..]);
    f.tick(CLAUDE, std::slice::from_ref(&path));
    assert_eq!(
        f.checkpoint(CLAUDE, &path).unwrap().state.project,
        Some(f.b)
    );
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &records)
    );
}

fn codex(minute: u32, kind: &str, payload: serde_json::Value) -> String {
    json!({"type": kind, "timestamp": at(minute, 0), "payload": payload}).to_string()
}

#[test]
fn observation_a_restart_keeps_the_folder_the_session_inherited() {
    // The rollout says where it is once, when it opens; it then works on
    // another project, and the shell is still judged by that folder.
    let exec = json!({"type": "custom_tool_call", "name": "exec", "input": "cargo test"});
    let patch = json!({"type": "custom_tool_call", "name": "apply_patch",
        "input": format!("*** Begin Patch\n*** Update File: {IN_B}/a.md\n*** End Patch")});
    let records = vec![
        codex(
            0,
            "session_meta",
            json!({"cwd": IN_A, "originator": "a desktop client"}),
        ),
        codex(1, "response_item", patch),
        codex(2, "response_item", exec.clone()),
        codex(3, "response_item", exec.clone()),
        codex(9, "event_msg", json!({"type": "task_complete"})),
    ];
    let f = Fixture::new();
    let path = f.file("rollout-session.jsonl");
    append(&path, &records[..2]);
    f.tick(CODEX, std::slice::from_ref(&path));
    let kept = f.checkpoint(CODEX, &path).unwrap();
    assert_eq!(
        (kept.state.project, kept.state.folder),
        (Some(f.b), Some(f.a))
    );
    append(&path, &records[2..]);
    f.tick(CODEX, std::slice::from_ref(&path));
    let after = f.checkpoint(CODEX, &path).unwrap();
    assert_eq!(
        after.state.project,
        Some(f.a),
        "two calls of the shell from its folder"
    );
    assert_eq!(after.last_kind, Kind::TurnEnd);
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CODEX, "rollout-session.jsonl", &records)
    );
}

#[test]
fn observation_a_restart_after_the_close_of_a_turn_keeps_the_session_waiting() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &session());
    f.tick(CLAUDE, std::slice::from_ref(&path));
    let (kept, checkpoints) = (f.kept(), f.dump("observation_checkpoints"));
    // A new process, a full pass, nothing new in the transcript: no row is
    // written, and the checkpoint still says the agent closed its turn.
    assert_eq!(
        f.live(CLAUDE)
            .tick(std::slice::from_ref(&path), &f.map, true)
            .unwrap(),
        0
    );
    assert_eq!(f.kept(), kept);
    assert_eq!(f.dump("observation_checkpoints"), checkpoints);
    assert_eq!(
        f.checkpoint(CLAUDE, &path).unwrap().last_kind,
        Kind::TurnEnd
    );
}

#[test]
fn observation_an_interruption_between_two_transcripts_gives_the_same_rows() {
    let other = vec![prompt(1, IN_B), said(3, IN_B), closed(7, IN_B)];
    let both = |f: &Fixture| (f.file("first.jsonl"), f.file("second.jsonl"));
    let whole = Fixture::new();
    let (first, second) = both(&whole);
    append(&first, &session());
    append(&second, &other);
    assert_eq!(whole.tick(CLAUDE, &[first, second]), 2);

    // The process stops after the first transcript's write.
    let cut = Fixture::new();
    let (first, second) = both(&cut);
    append(&first, &session());
    append(&second, &other);
    assert_eq!(cut.tick(CLAUDE, std::slice::from_ref(&first)), 1);
    assert_eq!(cut.tick(CLAUDE, &[first, second]), 1);
    assert_eq!(cut.kept(), whole.kept());
    assert_eq!(cut.prompts(), whole.prompts());
}

#[test]
fn observation_never_touches_admission() {
    use super::super::conversation::ConversationWatcher;
    use super::super::ingress::IngressTail;
    let run = |observe: bool| {
        let f = Fixture::new();
        let path = f.file("session.jsonl");
        append(&path, &session());
        if observe {
            f.tick(CLAUDE, std::slice::from_ref(&path));
        }
        let reader = IngressTail::new(f.storage.clone(), "conversation");
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        reader
            .tick_scoped(
                std::slice::from_ref(&path),
                &HashMap::new(),
                ConversationWatcher::parse_turn,
                None,
                false,
            )
            .unwrap();
        if observe {
            f.tick(CLAUDE, std::slice::from_ref(&path));
        }
        let file_id = |row: &String| row.split('|').collect::<Vec<_>>()[..3].join("|");
        (
            f.dump("ingest_cursors")
                .iter()
                .map(file_id)
                .collect::<Vec<_>>(),
            f.dump("ingest_events").len(),
            f.dump("work_stretches").len(),
            f.dump("work_segments").len(),
        )
    };
    let (with, without) = (run(true), run(false));
    assert_eq!(with.0, without.0, "the cursor of admission moved");
    assert_eq!((with.1, with.2), (without.1, without.2));
    assert_eq!((with.3, without.3), (2, 0));
}

#[test]
fn observation_a_transcript_a_program_ran_gets_segments_and_no_prompt() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    let program =
        |line: String| line.replace("\"entrypoint\":\"cli\"", "\"entrypoint\":\"sdk-py\"");
    let records: Vec<String> = session().into_iter().map(program).collect();
    append(&path, &records);
    assert_eq!(f.tick(CLAUDE, std::slice::from_ref(&path)), 1);
    let kept = f.kept();
    assert_eq!(kept.len(), 2);
    assert!(
        kept.iter().all(|row| row.contains("|program|claude|")),
        "{kept:?}"
    );
    assert!(f.prompts().is_empty());
    assert_eq!(f.checkpoint(CLAUDE, &path).unwrap().program, Some(true));
    // The pass admits nothing and makes no stretch, of anyone.
    for table in ["ingest_events", "ingest_cursors", "work_stretches"] {
        assert!(f.dump(table).is_empty(), "{table}");
    }
}

#[test]
fn observation_the_live_pass_leaves_history_to_the_backfill() {
    let f = Fixture::new();
    let path = f.file("old.jsonl");
    append(&path, &session());
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();
    let live = f.live(CLAUDE);
    assert_eq!(
        live.tick(std::slice::from_ref(&path), &f.map, true)
            .unwrap(),
        0
    );
    assert!(f.kept().is_empty() && f.checkpoint(CLAUDE, &path).is_none());
    // The backfill reads it, and from then on the live pass goes on in it.
    let backfill = f.live(CLAUDE).with_history();
    assert_eq!(
        backfill
            .tick(std::slice::from_ref(&path), &f.map, true)
            .unwrap(),
        1
    );
    assert_eq!(f.kept().len(), 2);
    append(&path, &[said(16, IN_A)]);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();
    assert_eq!(
        live.tick(std::slice::from_ref(&path), &f.map, true)
            .unwrap(),
        1
    );
}

#[test]
fn observation_no_table_of_the_pass_holds_a_folder_a_path_or_a_branch() {
    let f = Fixture::new();
    let claude_path = f.file("session.jsonl");
    append(&claude_path, &session());
    f.tick(CLAUDE, std::slice::from_ref(&claude_path));
    let codex_path = f.file("rollout-session.jsonl");
    append(
        &codex_path,
        &[
            codex(
                0,
                "session_meta",
                json!({"cwd": IN_A, "git": {"branch": format!("{SENTINEL}-branch")}}),
            ),
            codex(
                1,
                "response_item",
                json!({"type": "custom_tool_call", "name": "exec",
                "input": format!("ls {IN_B}")}),
            ),
        ],
    );
    f.tick(CODEX, std::slice::from_ref(&codex_path));
    let mut rows = 0;
    for table in [
        "work_segments",
        "segment_activity",
        "owner_prompts",
        "observation_checkpoints",
    ] {
        for row in f.dump(table) {
            rows += 1;
            assert!(!row.contains(SENTINEL), "{table} holds a location");
            assert!(!row.contains("/h/"), "{table} holds a path");
        }
    }
    assert!(rows >= 8);
}

#[test]
fn observation_a_transcript_admission_saw_unmoved_is_not_asked_of_the_file_system() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    let files = std::slice::from_ref(&path);
    let records = session();
    append(&path, &records[..2]);
    let pass = f.live(CLAUDE);
    let first = FileStamp::now(&path);
    assert_eq!(
        pass.tick_seen(files, &f.map, false, |_| first.clone())
            .unwrap(),
        1
    );
    let kept = f.kept();

    // The stamp admission gives is the one the pass read at: the pass
    // takes its word and does not look at the file, which has grown.
    append(&path, &records[2..]);
    assert_eq!(
        pass.tick_seen(files, &f.map, false, |_| first.clone())
            .unwrap(),
        0
    );
    assert_eq!(f.kept(), kept);
    // A full pass reads every transcript, whatever its stamp.
    assert_eq!(
        pass.tick_seen(files, &f.map, true, |_| first.clone())
            .unwrap(),
        1
    );
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &records)
    );

    // Where admission gives no stamp the pass asks the file system.
    append(&path, &[said(20, IN_A)]);
    assert_eq!(pass.tick_seen(files, &f.map, false, |_| None).unwrap(), 1);
    assert_eq!(pass.tick_seen(files, &f.map, false, |_| None).unwrap(), 0);
}

#[test]
fn observation_admission_gives_the_stamp_it_left_a_transcript_at() {
    use super::super::conversation::ConversationWatcher;
    use super::super::ingress::IngressTail;
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    let files = std::slice::from_ref(&path);
    append(&path, &session());
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    assert_eq!(reader.stamp_of(&path), None);
    let admit = || {
        let parse = ConversationWatcher::parse_turn;
        reader
            .tick_scoped(files, &HashMap::new(), parse, None, false)
            .unwrap();
    };
    let pass = f.live(CLAUDE);
    let observe = || {
        pass.tick_seen(files, &f.map, false, |path| reader.stamp_of(path))
            .unwrap()
    };
    admit();
    assert_eq!(reader.stamp_of(&path), FileStamp::now(&path));
    assert_eq!(observe(), 1);
    // Nothing moved: admission says so, and the pass reads nothing.
    admit();
    assert_eq!(observe(), 0);
    // What was appended is read by both in the tick that follows.
    append(&path, &[said(20, IN_A)]);
    admit();
    assert_eq!(reader.stamp_of(&path), FileStamp::now(&path));
    assert_eq!(observe(), 1);
}

#[test]
fn observation_a_merge_moves_no_row_and_undoing_it_gives_every_answer_as_before() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    let files = std::slice::from_ref(&path);
    // Begun in one project, a write into another, as `session` has it.
    append(&path, &session());
    let day = (
        DateTime::parse_from_rfc3339("2026-09-30T00:00:00Z")
            .unwrap()
            .to_utc(),
        DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .to_utc(),
    );
    let apart = BTreeMap::from([(Some(f.a), 300), (Some(f.b), 600)]);

    // Observed while the second project is merged into the first.
    f.storage.project_merge(f.b, f.a).unwrap();
    let mut merged = f.map.clone();
    merged.merge(f.b, f.a);
    assert_eq!(f.live(CLAUDE).tick(files, &merged, false).unwrap(), 1);
    // Each segment is kept under the folder its work was in, with the
    // times it has when nothing is merged.
    let spans = |f: &Fixture| {
        let conn = f.storage.conn.lock().unwrap();
        crate::test_support::rows(
            &conn,
            "SELECT project_id, started_at, last_at FROM work_segments ORDER BY started_at, id",
        )
    };
    assert_eq!(
        spans(&f),
        [
            format!("{}|2026-09-30 10:00:00|2026-09-30 10:05:00", f.a),
            format!("{}|2026-09-30 10:05:00|2026-09-30 10:15:00", f.b),
        ]
    );
    // It is served under the project it was merged into.
    assert_eq!(
        f.storage.agent_seconds(day.0, day.1).unwrap(),
        BTreeMap::from([(Some(f.a), 900)])
    );
    // Undone: every answer is what it is with no merge at all.
    assert!(f.storage.project_unmerge(f.b).unwrap());
    assert_eq!(f.storage.agent_seconds(day.0, day.1).unwrap(), apart);
    let never_merged = Fixture::new();
    let control_path = never_merged.file("session.jsonl");
    append(&control_path, &session());
    never_merged.tick(CLAUDE, std::slice::from_ref(&control_path));
    assert_eq!(spans(&f), spans(&never_merged));
}

#[test]
fn observation_a_pause_is_as_long_read_in_two_ticks_as_read_whole() {
    // 1,799.2 seconds apart by the fractions the records carry, 1,800 by
    // the seconds the store keeps: the idle time, and a new stretch.
    let stamped = |line: String, time: &str| {
        let mut record: serde_json::Value = serde_json::from_str(&line).unwrap();
        record["timestamp"] = time.into();
        record.to_string()
    };
    let records = vec![
        stamped(prompt(0, IN_A), "2026-09-30T10:00:00.900Z"),
        stamped(said(30, IN_A), "2026-09-30T10:30:00.100Z"),
        stamped(said(31, IN_A), "2026-09-30T10:31:00.500Z"),
    ];
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    for record in &records {
        append(&path, std::slice::from_ref(record));
        f.tick(CLAUDE, std::slice::from_ref(&path));
    }
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &records)
    );
    assert_eq!(f.kept().len(), 2);
}

#[test]
fn observation_rows_written_before_the_driver_was_known_are_made_what_a_whole_read_gives() {
    // The records of a session a program ran; the first two do not say
    // how it was started.
    let unsaid = |line: String| {
        let mut record: serde_json::Value = serde_json::from_str(&line).unwrap();
        record.as_object_mut().unwrap().remove("entrypoint");
        record.to_string()
    };
    let by_program = |line: String| {
        let mut record: serde_json::Value = serde_json::from_str(&line).unwrap();
        record["entrypoint"] = "sdk-py".into();
        record.to_string()
    };
    let records = vec![
        unsaid(prompt(0, IN_A)),
        unsaid(said(1, IN_A)),
        by_program(said(4, IN_A)),
        by_program(closed(9, IN_A)),
    ];
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    append(&path, &records[..2]);
    f.tick(CLAUDE, std::slice::from_ref(&path));
    // Read so far, it is taken for the owner's, with the owner's prompt.
    assert!(f.kept()[0].contains("|owner|") && f.prompts().len() == 1);
    f.storage
        .attribution_dirty_clear(&f.storage.attribution_dirty().unwrap())
        .unwrap();
    append(&path, &records[2..]);
    f.tick(CLAUDE, std::slice::from_ref(&path));
    assert!(f.kept()[0].contains("|program|") && f.prompts().is_empty());
    assert_eq!(
        (f.kept(), f.prompts()),
        control(CLAUDE, "session.jsonl", &records)
    );
    // The owner's time of that day is made again: a session left it.
    assert_eq!(f.storage.attribution_dirty().unwrap().len(), 1);

    // Said in the same pass, after a prompt was read: no prompt is kept.
    let whole = Fixture::new();
    let path = whole.file("session.jsonl");
    append(&path, &records);
    whole.tick(CLAUDE, std::slice::from_ref(&path));
    assert!(whole.prompts().is_empty());
}

#[test]
fn observation_a_record_copied_from_before_is_not_the_last_thing_a_session_did() {
    let f = Fixture::new();
    let path = f.file("session.jsonl");
    // A turn closed at minute 10; a compaction then copies the prompt of
    // minute 0 to the end of the file.
    append(&path, &[prompt(0, IN_A), closed(10, IN_A), prompt(0, IN_A)]);
    f.tick(CLAUDE, std::slice::from_ref(&path));
    let checkpoint = f.checkpoint(CLAUDE, &path).unwrap();
    assert_eq!(checkpoint.last_kind, Kind::TurnEnd);
    assert_eq!(
        checkpoint.last_at.unwrap().to_rfc3339(),
        "2026-09-30T10:10:00+00:00"
    );
    // A record with no time says nothing of when the session last moved.
    append(
        &path,
        &[json!({"type": "summary", "summary": "a title"}).to_string()],
    );
    f.tick(CLAUDE, std::slice::from_ref(&path));
    assert_eq!(
        f.checkpoint(CLAUDE, &path).unwrap().last_kind,
        Kind::TurnEnd
    );
}

/// A signal that goes nowhere.
struct Silent;

impl crate::live_signal::Post for Silent {
    fn post(&self, _name: &str) {}
}

#[test]
fn live_two_transcripts_on_two_projects_are_two_live_projects_with_their_agents() {
    let f = Fixture::new();
    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    live.load_projects(&f.storage).unwrap();
    // The records are of now: a session is live by the age of its last.
    let now = chrono::Utc::now();
    let stamped = |line: String, seconds_ago: i64| {
        let mut record: serde_json::Value = serde_json::from_str(&line).unwrap();
        let at = now - chrono::Duration::seconds(seconds_ago);
        record["timestamp"] = at.to_rfc3339().into();
        record.to_string()
    };
    // The owner asked in one project; the agent closed its turn.
    let claude = f.file("session.jsonl");
    append(
        &claude,
        &[stamped(prompt(0, IN_A), 50), stamped(closed(0, IN_A), 20)],
    );
    // A rollout in the other project, still at work.
    let rollout = f.file("rollout-a.jsonl");
    append(
        &rollout,
        &[
            stamped(
                codex(
                    0,
                    "session_meta",
                    json!({"cwd": IN_B, "originator": "a client"}),
                ),
                40,
            ),
            stamped(
                codex(
                    0,
                    "response_item",
                    json!({"type": "custom_tool_call", "name": "exec",
                    "input": format!("ls {IN_B}")}),
                ),
                10,
            ),
        ],
    );
    let pass =
        |format| Observation::new(f.storage.clone(), format, IDLE).with_live(Some(live.clone()));
    assert_eq!(
        pass(CLAUDE)
            .tick(std::slice::from_ref(&claude), &f.map, false)
            .unwrap(),
        1
    );
    assert_eq!(
        pass(CODEX)
            .tick(std::slice::from_ref(&rollout), &f.map, false)
            .unwrap(),
        1
    );

    let snapshot: serde_json::Value = serde_json::from_slice(&live.state.snapshot().1).unwrap();
    let projects = snapshot["projects"].as_array().unwrap();
    let shown: Vec<(&str, &str, &str)> = projects
        .iter()
        .map(|project| {
            (
                project["key"].as_str().unwrap(),
                project["sessions"][0]["agent"].as_str().unwrap(),
                project["sessions"][0]["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("notes", "codex", "running"),
            ("demoapp", "claude", "waiting")
        ]
    );
    // Nothing of where a session is reaches the snapshot.
    let served = String::from_utf8(live.state.snapshot().1.to_vec()).unwrap();
    assert!(!served.contains(SENTINEL) && !served.contains("/h/"));

    // More records of the running session change nothing that is shown.
    let version = live.state.version();
    append(
        &rollout,
        &[stamped(
            codex(
                0,
                "response_item",
                json!({"type": "message",
        "role": "assistant", "content": []}),
            ),
            5,
        )],
    );
    pass(CODEX)
        .tick(std::slice::from_ref(&rollout), &f.map, true)
        .unwrap();
    assert_eq!(live.state.version(), version);
}

#[test]
fn live_a_profile_with_no_workspace_shows_its_sessions_under_unknown() {
    let f = Fixture::new();
    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    let now = chrono::Utc::now();
    let stamped = |line: String, seconds_ago: i64| {
        let mut record: serde_json::Value = serde_json::from_str(&line).unwrap();
        let at = now - chrono::Duration::seconds(seconds_ago);
        record["timestamp"] = at.to_rfc3339().into();
        record.to_string()
    };
    let claude = f.file("session.jsonl");
    append(
        &claude,
        &[stamped(prompt(0, IN_A), 50), stamped(said(0, IN_A), 40)],
    );
    let rollout = f.file("rollout-a.jsonl");
    let meta = json!({"cwd": IN_B, "originator": "codex_exec", "source": "exec"});
    append(
        &rollout,
        &[
            stamped(codex(0, "session_meta", meta), 30),
            stamped(codex(0, "event_msg", json!({"type": "task_complete"})), 10),
        ],
    );
    // A transcript nobody wrote to for the idle time is not read.
    let old = f.file("old.jsonl");
    append(&old, &[stamped(prompt(0, IN_A), 5)]);
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(IDLE + 60);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();

    let pass =
        |format| Observation::new(f.storage.clone(), format, IDLE).with_live(Some(live.clone()));
    let claude_pass = pass(CLAUDE);
    let files = [claude.clone(), old.clone()];
    assert_eq!(
        claude_pass.tick_unstored(&files, false, |_| None).unwrap(),
        2
    );
    pass(CODEX)
        .tick_unstored(std::slice::from_ref(&rollout), false, |_| None)
        .unwrap();

    let snapshot = |live: &crate::live_run::Live| -> serde_json::Value {
        serde_json::from_slice(&live.state.snapshot().1).unwrap()
    };
    let shown = snapshot(&live);
    assert_eq!(shown["projects"], json!([]));
    let sessions = shown["unknown"]["sessions"].as_array().unwrap();
    let told: Vec<(&str, &str, &str)> = sessions
        .iter()
        .map(|session| {
            (
                session["agent"].as_str().unwrap(),
                session["driver"].as_str().unwrap(),
                session["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        told,
        [
            ("claude", "owner", "running"),
            ("codex", "program", "waiting")
        ]
    );
    // Nothing was written: no segment, no prompt, no checkpoint.
    assert!(f.kept().is_empty() && f.prompts().is_empty());
    assert!(f.dump("observation_checkpoints").is_empty());

    // What is appended is read from where the reading stopped: the agent
    // closes its turn, and the session waits for the owner.
    append(&claude, &[stamped(closed(0, IN_A), 1)]);
    assert_eq!(
        claude_pass.tick_unstored(&files, false, |_| None).unwrap(),
        1
    );
    let shown = snapshot(&live);
    assert_eq!(shown["unknown"]["sessions"][0]["state"], "waiting");
    // Unchanged files are not read again.
    assert_eq!(
        claude_pass.tick_unstored(&files, false, |_| None).unwrap(),
        0
    );
}

#[test]
fn live_a_transcript_rewritten_in_place_is_read_anew_by_the_pass_that_stores_nothing() {
    let f = Fixture::new();
    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    let now = chrono::Utc::now();
    let stamped = |line: String, seconds_ago: i64| {
        let mut record: serde_json::Value = serde_json::from_str(&line).unwrap();
        let at = now - chrono::Duration::seconds(seconds_ago);
        record["timestamp"] = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true).into();
        record
    };
    // Two files of the same length: in one the agent is at work, in the
    // other it closed its turn.
    let working = stamped(said(0, IN_A), 20);
    let mut closing = stamped(closed(0, IN_A), 20);
    let pad = closing.to_string().len() - working.to_string().len();
    let mut working = working;
    working["message"]["content"][0]["text"] = format!("looking{}", "!".repeat(pad)).into();
    closing["message"]["content"][0]["text"] = "looking".into();
    let (working, closing) = (working.to_string(), closing.to_string());
    assert_eq!(working.len(), closing.len());

    let path = f.file("session.jsonl");
    let files = std::slice::from_ref(&path);
    append(&path, std::slice::from_ref(&working));
    let pass = Observation::new(f.storage.clone(), CLAUDE, IDLE).with_live(Some(live.clone()));
    assert_eq!(pass.tick_unstored(files, false, |_| None).unwrap(), 1);
    let state = |live: &crate::live_run::Live| -> serde_json::Value {
        let snapshot: serde_json::Value = serde_json::from_slice(&live.state.snapshot().1).unwrap();
        snapshot["unknown"]["sessions"][0]["state"].clone()
    };
    assert_eq!(state(&live), "running");

    // Rewritten in place: the same file, the same length, other bytes,
    // and the time of its change put back, so its stamp says nothing.
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    writeln!(file, "{closing}").unwrap();
    file.set_modified(before).unwrap();
    drop(file);
    assert_eq!(
        FileStamp::now(&path),
        Some(FileStamp::of(&std::fs::metadata(&path).unwrap()))
    );
    // A pass that goes by the stamp does not look; a full pass checks
    // what it read against the file, and reads it anew.
    assert_eq!(pass.tick_unstored(files, false, |_| None).unwrap(), 0);
    assert_eq!(state(&live), "running");
    assert_eq!(pass.tick_unstored(files, true, |_| None).unwrap(), 1);
    assert_eq!(state(&live), "waiting");

    // Rewritten with a new time of change: the next pass sees it.
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    writeln!(file, "{working}").unwrap();
    file.set_modified(before + std::time::Duration::from_secs(5))
        .unwrap();
    drop(file);
    assert_eq!(pass.tick_unstored(files, false, |_| None).unwrap(), 1);
    assert_eq!(state(&live), "running");
}

/// How fast the pass reads new bytes, and what that is beside the tick of
/// admission over the same bytes. Run with
/// `cargo test --release observation_benchmark -- --ignored --nocapture`.
///
/// The pass reads the bytes a second time and parses every record, so it
/// cannot cost a twentieth of admission: it is held to a rate instead, the
/// way the resolver it runs is (live-projects, design 3.2 and 19).
/// Live state is attached: the timed pass also reads and retains metadata
/// for plans, then publishes the session after its transaction succeeds.
#[test]
#[ignore]
fn observation_benchmark_reads_400_mb_a_second_or_more() {
    use super::super::conversation::ConversationWatcher;
    use super::super::ingress::IngressTail;
    let result = "a line of output that a tool printed, with nothing to find\n".repeat(140);
    let lines = [
        prompt(0, IN_A),
        tool(1, IN_A, "Edit", "notes"),
        claude(
            2,
            IN_A,
            "user",
            json!([{"type": "tool_result", "content": result}]),
        ),
        said(3, IN_A),
    ];
    let mut text = String::new();
    while text.len() < 48 << 20 {
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
    }
    let time = |observe: bool| -> f64 {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let f = Fixture::new();
            let path = f.file("session.jsonl");
            std::fs::write(&path, &text).unwrap();
            let reader = IngressTail::new(f.storage.clone(), "conversation");
            reader.bootstrap(&[], &HashMap::new()).unwrap();
            let live =
                crate::live_run::Live::new("benchmark", "test signal".into(), Box::new(Silent));
            live.load_projects(&f.storage).unwrap();
            let pass = f.live(CLAUDE).with_live(Some(live.clone()));
            let began = std::time::Instant::now();
            reader
                .tick_scoped(
                    std::slice::from_ref(&path),
                    &HashMap::new(),
                    ConversationWatcher::parse_turn,
                    None,
                    false,
                )
                .unwrap();
            if observe {
                assert_eq!(
                    pass.tick(std::slice::from_ref(&path), &f.map, false)
                        .unwrap(),
                    1
                );
            }
            let elapsed = began.elapsed().as_secs_f64();
            if observe {
                let stream = stream_of(CLAUDE.namespace, &path);
                let location = live.location(&stream).expect("live location was observed");
                assert_eq!(location.cwd.as_deref(), Some(Path::new(IN_A)));
                assert_eq!(
                    location.branch.as_deref(),
                    Some(format!("{SENTINEL}-branch").as_str())
                );
            }
            best = best.min(elapsed);
        }
        best
    };
    let (alone, with) = (time(false), time(true));
    let megabytes = text.len() as f64 / 1e6;
    let rate = megabytes / (with - alone);
    eprintln!(
        "{megabytes:.0} MB of new records with live locations: admission {:.0} ms, the observation pass {:.0} ms on top \
         (+{:.1}%), {rate:.0} MB a second",
        alone * 1e3,
        (with - alone) * 1e3,
        100.0 * (with - alone) / alone
    );
    assert!(rate >= 400.0, "the pass read {rate:.0} MB a second");
}
