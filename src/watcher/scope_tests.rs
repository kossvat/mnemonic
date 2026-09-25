//! Project scope through the real durable reader, with no daemon and no sleeps.
use super::*;
use crate::storage::Storage;
use crate::watcher::codex::CodexWatcher;
use crate::watcher::conversation::ConversationWatcher;
use crate::watcher::ingress::IngressTail;
use std::collections::HashMap as Map;
use std::io::Write;
use std::sync::Arc;

struct Fixture {
    dir: PathBuf,
    storage: Arc<Storage>,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mn-scoped-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
        Self { dir, storage }
    }
    /// Contents of every captured, not yet consumed, turn.
    fn captured(&self) -> Vec<String> {
        self.storage
            .pending_ingest(chrono::Utc::now(), 100)
            .unwrap()
            .into_iter()
            .map(|pending| pending.event.content)
            .collect()
    }
    /// A file that exists before the reader starts is history: adopt it the
    /// way the daemon does, so only what is appended afterwards is new.
    fn adopt(&self, reader: &IngressTail, path: &Path) {
        reader
            .bootstrap(std::slice::from_ref(&path.to_path_buf()), &Map::new())
            .unwrap();
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

fn demo() -> ProjectScope {
    ProjectScope::new(&[PathBuf::from("/code/demoapp")])
}

fn claude(cwd: Option<&str>, id: &str, text: &str) -> String {
    let mut value = serde_json::json!({"type": "user", "uuid": id, "sessionId": "s",
        "timestamp": "2026-09-22T10:00:00Z", "message": {"content": text}});
    if let Some(cwd) = cwd {
        value["cwd"] = cwd.into();
    }
    value.to_string()
}

fn codex_user(id: &str, text: &str) -> String {
    serde_json::json!({"type": "response_item", "timestamp": "2026-09-22T10:00:00Z",
        "payload": {"type": "message", "role": "user", "id": id,
                    "content": [{"type": "input_text", "text": text}]}})
    .to_string()
}
fn meta(cwd: &str) -> String {
    serde_json::json!({"type": "session_meta", "payload": {"cwd": cwd}}).to_string()
}
fn turn(cwd: &str) -> String {
    serde_json::json!({"type": "turn_context", "payload": {"cwd": cwd}}).to_string()
}
fn done() -> String {
    serde_json::json!({"type": "event_msg", "payload": {"type": "task_complete"}}).to_string()
}

#[test]
fn a_claude_session_counts_by_the_folder_it_was_launched_in() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    let mut scope = ClaudeScope::new(demo());
    let session = |name: &str, lines: &[String]| {
        let path = f.dir.join(name);
        std::fs::write(&path, "").unwrap();
        f.adopt(&reader, &path);
        append(&path, lines);
        path
    };

    // Launched in the project, then working in a subfolder: all of it counts.
    let inside = session(
        "inside.jsonl",
        &[
            claude(Some("/code/demoapp"), "m1", "это не то, цену ставим 42"),
            claude(
                Some("/code/demoapp/api"),
                "m2",
                "это не то, эндпоинт переименуем",
            ),
        ],
    );
    // Launched in a monorepo that holds every project, and the agent then
    // `cd`s into the project folder: the records now name the project, but
    // the session is the owner's monorepo session and must stay out.
    let drifted = session(
        "drifted.jsonl",
        &[
            claude(Some("/code"), "m3", "это не то, общий вопрос по монорепо"),
            claude(Some("/code/demoapp"), "m4", "это не то, секрет из монорепо"),
        ],
    );
    // No folder on the record: nothing to judge by.
    let bare = session(
        "bare.jsonl",
        &[claude(None, "m5", "это не то, запись без папки")],
    );
    for path in [&inside, &drifted, &bare] {
        reader
            .poll_file_scoped(path, ConversationWatcher::parse_turn, &mut scope)
            .unwrap();
    }
    let mut captured = f.captured();
    captured.sort();
    assert_eq!(
        captured,
        vec![
            "это не то, цену ставим 42".to_owned(),
            "это не то, эндпоинт переименуем".to_owned(),
        ]
    );

    // Refused records are consumed, not retried on the next poll.
    let again = reader
        .poll_file_scoped(&drifted, ConversationWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(again, 0);
}

#[test]
fn a_restarted_daemon_recovers_the_launch_folder_of_a_claude_session() {
    for (launched, expect_captured) in [("/code", false), ("/code/demoapp", true)] {
        let f = Fixture::new();
        let path = f.dir.join("session.jsonl");
        std::fs::write(&path, "").unwrap();
        {
            let reader = IngressTail::new(f.storage.clone(), "conversation");
            f.adopt(&reader, &path);
            let mut scope = ClaudeScope::new(demo());
            append(&path, &[claude(Some(launched), "m1", "как дела")]);
            reader
                .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut scope)
                .unwrap();
        }
        let reader = IngressTail::new(f.storage.clone(), "conversation");
        let mut scope = ClaudeScope::new(demo());
        append(
            &path,
            &[claude(
                Some("/code/demoapp"),
                "m2",
                "это не то, после перезапуска",
            )],
        );
        reader
            .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut scope)
            .unwrap();
        assert_eq!(
            !f.captured().is_empty(),
            expect_captured,
            "session launched in {launched}"
        );
    }
}

#[test]
fn a_codex_session_is_judged_by_the_folder_of_each_turn() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "codex");
    let path = f.dir.join("rollout-moved.jsonl");
    std::fs::write(&path, "").unwrap();
    f.adopt(&reader, &path);
    let mut scope = CodexScope::new(demo());

    append(
        &path,
        &[
            meta("/code/demoapp"),
            codex_user("u1", "это не то, решение по DemoApp"),
            done(),
            // Resumed elsewhere: the injected text comes before the new
            // turn_context, and must not ride on the previous folder.
            codex_user("u2", "это не то, инструкции чужого проекта"),
            turn("/code/private"),
            codex_user("u3", "это не то, секрет после переезда"),
            done(),
            turn("/code/demoapp"),
            codex_user("u4", "это не то, снова в DemoApp"),
        ],
    );
    reader
        .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(
        f.captured(),
        vec![
            "это не то, решение по DemoApp".to_owned(),
            "это не то, снова в DemoApp".to_owned(),
        ]
    );
}

#[test]
fn a_restarted_daemon_recovers_the_folder_from_the_part_it_already_read() {
    for (moved_to, expect_captured) in [("/code/private", false), ("/code/demoapp", true)] {
        let f = Fixture::new();
        let path = f.dir.join("rollout-restart.jsonl");
        std::fs::write(&path, "").unwrap();
        {
            let reader = IngressTail::new(f.storage.clone(), "codex");
            f.adopt(&reader, &path);
            let mut scope = CodexScope::new(demo());
            let start = if moved_to == "/code/private" {
                "/code/demoapp"
            } else {
                "/code/private"
            };
            append(&path, &[meta(start), done(), turn(moved_to)]);
            reader
                .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
                .unwrap();
        }
        // A new process: fresh reader, fresh scope, same durable cursor.
        let reader = IngressTail::new(f.storage.clone(), "codex");
        let mut scope = CodexScope::new(demo());
        append(&path, &[codex_user("u1", "это не то, после перезапуска")]);
        reader
            .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
            .unwrap();
        assert_eq!(
            !f.captured().is_empty(),
            expect_captured,
            "session last moved to {moved_to}"
        );
    }
}

#[test]
fn a_rewritten_codex_transcript_starts_from_its_new_header() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "codex");
    let path = f.dir.join("rollout-rewrite.jsonl");
    std::fs::write(&path, "").unwrap();
    f.adopt(&reader, &path);
    let mut scope = CodexScope::new(demo());

    append(
        &path,
        &[
            meta("/code/demoapp"),
            codex_user("u1", "это не то, первая версия файла"),
        ],
    );
    reader
        .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(f.captured().len(), 1);

    // Replaced by a shorter transcript of another project.
    std::fs::write(
        &path,
        format!("{}\n{}\n", meta("/p"), codex_user("u9", "это не то, чужое")),
    )
    .unwrap();
    reader
        .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(f.captured().len(), 1, "the other project must not come in");
}

#[test]
fn an_uncommitted_poll_does_not_move_the_folder_forward() {
    let dir =
        std::env::temp_dir().join(format!("mn-uncommitted-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout.jsonl");
    std::fs::write(&path, "").unwrap();
    let mut file = std::fs::File::open(&path).unwrap();
    let mut scope = CodexScope::new(demo());

    scope.begin(&mut file, &path, 0, 0).unwrap();
    assert!(scope.admit(&meta("/code/demoapp")));
    scope.commit(&path, 0, 100);

    // A poll that read further and then failed before committing.
    scope.begin(&mut file, &path, 0, 100).unwrap();
    scope.admit(&done());
    scope.admit(&turn("/code/private"));

    // The retry starts where the cursor still is, in DemoApp.
    scope.begin(&mut file, &path, 0, 100).unwrap();
    assert!(scope.admit(&codex_user("u1", "x")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_replay_reads_the_file_being_polled_not_what_the_path_names_now() {
    let dir = std::env::temp_dir().join(format!("mn-replaced-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout.jsonl");
    // Longer than the replacement's header, so a replay of the replacement
    // would read that header whole and wrongly find the project.
    let head = format!("{}\n", meta("/code/private/a-longer-folder"));
    std::fs::write(&path, format!("{head}{}\n", codex_user("u1", "x"))).unwrap();
    let mut polled = std::fs::File::open(&path).unwrap();

    // Between the reader opening the file and the scope replaying it, the
    // path is replaced by a transcript of the project.
    let other = dir.join("replacement.jsonl");
    let replacement = format!("{}\n", meta("/code/demoapp"));
    assert!(replacement.len() <= head.len());
    std::fs::write(&other, replacement).unwrap();
    std::fs::rename(&other, &path).unwrap();

    // A fresh process, resuming the polled file after its header.
    let mut scope = CodexScope::new(demo());
    scope
        .begin(&mut polled, &path, 0, head.len() as u64)
        .unwrap();
    assert!(
        !scope.admit(&codex_user("u1", "x")),
        "the records come from the private rollout, so must its folder"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

fn started() -> String {
    serde_json::json!({"type": "event_msg", "payload": {"type": "task_started"}}).to_string()
}

#[test]
fn a_codex_turn_that_never_finished_does_not_carry_its_folder_into_a_resume() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "codex");
    let path = f.dir.join("rollout-crash.jsonl");
    std::fs::write(&path, "").unwrap();
    f.adopt(&reader, &path);
    let mut scope = CodexScope::new(demo());

    append(
        &path,
        &[
            meta("/code/demoapp"),
            started(),
            turn("/code/demoapp"),
            codex_user("u1", "это не то, решение по DemoApp"),
            // Crash: no task_complete. Resumed later in another folder, which
            // writes its injected instructions before its turn_context.
            started(),
            codex_user("u2", "это не то, AGENTS.md чужого проекта"),
            turn("/code/private"),
            codex_user("u3", "это не то, секрет чужого проекта"),
        ],
    );
    reader
        .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(
        f.captured(),
        vec!["это не то, решение по DemoApp".to_owned()]
    );
}

#[test]
fn a_codex_exec_session_without_turn_context_still_counts() {
    // `codex exec` writes session_meta and task_started, and no turn_context.
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "codex");
    let path = f.dir.join("rollout-exec.jsonl");
    std::fs::write(&path, "").unwrap();
    f.adopt(&reader, &path);
    let mut scope = CodexScope::new(demo());
    append(
        &path,
        &[
            meta("/code/demoapp"),
            started(),
            codex_user("u1", "это не то, запуск из exec"),
            done(),
        ],
    );
    reader
        .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(f.captured(), vec!["это не то, запуск из exec".to_owned()]);
}

#[test]
fn the_production_tick_applies_the_scope() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    let mut scope = ClaudeScope::new(demo());
    // Sessions created after startup: the tick reads them from the top.
    let files = vec![f.dir.join("inside.jsonl"), f.dir.join("outside.jsonl")];
    reader
        .tick_scoped(&files, &Map::new(), ConversationWatcher::parse_turn, None)
        .unwrap();
    append(
        &files[0],
        &[claude(
            Some("/code/demoapp"),
            "t1",
            "это не то, цену ставим 42",
        )],
    );
    append(
        &files[1],
        &[claude(
            Some("/code/private"),
            "t2",
            "это не то, чужой секрет",
        )],
    );
    let captured = reader
        .tick_scoped(
            &files,
            &Map::new(),
            ConversationWatcher::parse_turn,
            Some(&mut scope),
        )
        .unwrap();
    assert_eq!(captured, 1);
    assert_eq!(f.captured(), vec!["это не то, цену ставим 42".to_owned()]);
}
