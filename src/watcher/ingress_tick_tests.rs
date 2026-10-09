//! The tick at rest: an unchanged transcript is not opened, a changed one is
//! read in the tick it changed, through the same admission as before.
use super::*;
use crate::watcher::conversation::ConversationWatcher;
use crate::watcher::discovery::Discovery;
use std::io::{Seek, SeekFrom, Write};

struct Fixture {
    dir: PathBuf,
    storage: Arc<Storage>,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mn-tick-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
        Self { dir, storage }
    }
    fn captured(&self) -> Vec<String> {
        self.storage
            .pending_ingest(Utc::now(), 10_000)
            .unwrap()
            .into_iter()
            .map(|pending| pending.event.content)
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn record(id: &str, text: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({"type": "user", "uuid": id, "sessionId": "demoapp-session",
            "timestamp": "2026-09-20T10:00:00Z", "message": {"content": text}})
    )
}

fn append(path: &Path, text: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

const PARSE: fn(&Path, &str) -> Option<ParsedTurn> = ConversationWatcher::parse_turn;
const NONE: fn() -> HashMap<PathBuf, u64> = HashMap::new;

#[test]
fn ingress_tick_unchanged_transcripts_are_not_opened() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    let files: Vec<PathBuf> = (0..2_500)
        .map(|n| {
            let path = f.dir.join(format!("session-{n:04}.jsonl"));
            append(&path, &record(&format!("m{n}"), "Decision: use SQLite"));
            path
        })
        .collect();
    // The first tick adopts every transcript as history and reads each.
    reader
        .tick_scoped(&files, &NONE(), PARSE, None, true)
        .unwrap();
    let opened = reader.opened();
    assert!(opened >= files.len());

    for _ in 0..3 {
        assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 0);
    }
    assert_eq!(reader.opened(), opened, "an idle tick opened a transcript");

    // A full pass reads every one again, as every tick did before.
    reader
        .tick_scoped(&files, &NONE(), PARSE, None, true)
        .unwrap();
    assert_eq!(reader.opened(), opened + files.len());
    assert!(f.captured().is_empty(), "history stays history");
}

#[test]
fn ingress_tick_a_transcript_that_grew_is_read_in_that_tick() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    reader.bootstrap(&[], &NONE()).unwrap();
    let path = f.dir.join("session.jsonl");
    let quiet = f.dir.join("quiet.jsonl");
    append(&path, &record("m1", "Decision: use SQLite"));
    append(&quiet, &record("q1", "Decision: use Redis"));
    let files = vec![path.clone(), quiet.clone()];
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 2);

    let before = reader.opened();
    append(&path, &record("m2", "stop, use the other configuration"));
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 1);
    assert_eq!(reader.opened(), before + 1, "only the transcript that grew");
    assert_eq!(
        f.captured(),
        vec![
            "Decision: use SQLite".to_owned(),
            "Decision: use Redis".to_owned(),
            "stop, use the other configuration".to_owned(),
        ]
    );
}

#[test]
fn ingress_tick_an_old_transcript_that_grows_after_a_full_pass_is_read_next_tick() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    reader.bootstrap(&[], &NONE()).unwrap();
    let path = f.dir.join("old.jsonl");
    append(&path, &record("m1", "Decision: use SQLite"));
    let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(two_days_ago)
        .unwrap();
    let files = vec![path.clone()];
    assert_eq!(
        reader
            .tick_scoped(&files, &NONE(), PARSE, None, true)
            .unwrap(),
        1
    );
    // The session is resumed right after the pass.
    append(&path, &record("m2", "Decision: use Postgres"));
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 1);
}

#[test]
fn ingress_tick_a_new_file_in_a_known_folder_is_read_within_one_tick() {
    let f = Fixture::new();
    let root = f.dir.join("projects");
    let project = root.join("-code-demoapp");
    std::fs::create_dir_all(&project).unwrap();
    append(
        &project.join("a.jsonl"),
        &record("a1", "Decision: use SQLite"),
    );
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    let mut discovery = Discovery::default();

    let files = discovery.claude_transcripts(&root);
    reader
        .tick_scoped(&files, &NONE(), PARSE, None, true)
        .unwrap();
    let listed = discovery.listed;
    // Nothing moved: the folders are stated, not listed again.
    let files = discovery.claude_transcripts(&root);
    assert_eq!(files.len(), 1);
    assert_eq!(discovery.listed, listed);

    append(
        &project.join("b.jsonl"),
        &record("b1", "Decision: use Redis"),
    );
    let files = discovery.claude_transcripts(&root);
    assert_eq!(files.len(), 2);
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 1);
    assert_eq!(f.captured(), vec!["Decision: use Redis".to_owned()]);

    // A new project folder is found the same way.
    let other = root.join("-code-other");
    std::fs::create_dir_all(&other).unwrap();
    append(
        &other.join("c.jsonl"),
        &record("c1", "Decision: use Postgres"),
    );
    let files = discovery.claude_transcripts(&root);
    assert_eq!(files.len(), 3);
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 1);
}

#[test]
fn ingress_tick_a_failed_read_is_tried_again_though_its_stamp_did_not_move() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    reader.bootstrap(&[], &NONE()).unwrap();
    let path = f.dir.join("session.jsonl");
    append(&path, &record("m1", "Decision: use SQLite"));
    let files = vec![path.clone()];
    // The store refuses the first cursor of this transcript.
    let conn = || f.storage.conn.lock().unwrap();
    conn()
        .execute_batch(
            "CREATE TRIGGER test_block_cursor BEFORE INSERT ON ingest_cursors
             BEGIN SELECT RAISE(ABORT, 'injected store failure'); END;",
        )
        .unwrap();
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 0);
    conn()
        .execute_batch("DROP TRIGGER test_block_cursor")
        .unwrap();

    let before = reader.opened();
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 1);
    assert_eq!(reader.opened(), before + 1);
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 0);
    assert_eq!(reader.opened(), before + 1);
}

#[test]
fn ingress_tick_a_rewrite_that_keeps_length_and_time_waits_for_the_full_pass() {
    let f = Fixture::new();
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    reader.bootstrap(&[], &NONE()).unwrap();
    let path = f.dir.join("session.jsonl");
    append(&path, &record("m1", "Decision: use SQLite"));
    let files = vec![path.clone()];
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 1);
    let stream = reader.stream(&path);
    let generation = |f: &Fixture| {
        f.storage
            .ingest_cursor(&stream)
            .unwrap()
            .unwrap()
            .generation
    };
    assert_eq!(generation(&f), 0);

    // The same bytes but one, in place, and the change time put back.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let at = text.find("\"m1\"").unwrap() + 2;
    let mut file = std::fs::File::options().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(at as u64)).unwrap();
    file.write_all(b"2").unwrap();
    file.set_modified(modified).unwrap();
    drop(file);

    let before = reader.opened();
    assert_eq!(reader.tick(&files, &NONE(), PARSE).unwrap(), 0);
    assert_eq!(reader.opened(), before, "the stamp did not move");
    assert_eq!(generation(&f), 0);

    // The next full pass sees the rewrite by its fingerprint, as before.
    assert_eq!(
        reader
            .tick_scoped(&files, &NONE(), PARSE, None, true)
            .unwrap(),
        1
    );
    assert_eq!(generation(&f), 1);
}
