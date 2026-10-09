//! Real process: the commands of the first section of the runbook of
//! live-projects, on a scratch profile. The backup copies a store as it
//! is; the backfill reads history into segments and can be run again.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = common::short_root().join(format!("mn-bf-{tag}-{}", &id[..8]));
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mnemonic(user_home: &Path, profile: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
    cmd.env("HOME", user_home)
        .env_remove("MNEMONIC_HOME")
        .env("FASTEMBED_CACHE_DIR", user_home.join(".fastembed_cache"))
        // The loopback address, written so that the scanner of the public
        // export does not read it as an address of a host.
        .env(
            "HF_ENDPOINT",
            format!("http://{}:9", ["127", "0", "0", "1"].join(".")),
        )
        .env_remove("HF_HOME");
    for proxy in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
        cmd.env_remove(proxy).env_remove(proxy.to_lowercase());
    }
    if let Some(profile) = profile {
        cmd.arg("--home").arg(profile);
    }
    cmd.args(args).output().unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

/// A profile whose store exists, and the home it runs under.
fn profile(tag: &str) -> (Temp, PathBuf, PathBuf) {
    let root = Temp::new(tag);
    let user_home = root.0.join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.0.join("client");
    stdout(&mnemonic(&user_home, Some(&profile), &["init"]));
    stdout(&mnemonic(
        &user_home,
        Some(&profile),
        &["save", "--title", "a note", "kept as it is"],
    ));
    (root, user_home, profile)
}

fn generation(db: &Path) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[test]
fn backfill_cli_the_backup_copies_the_stores_as_they_are() {
    let (root, user_home, profile) = profile("backup");
    let store = profile.join("memory.db");
    // A store of the generation before this binary's.
    let earlier = generation(&store) - 1;
    rusqlite::Connection::open(&store)
        .unwrap()
        .pragma_update(None, "user_version", earlier)
        .unwrap();
    let into = root.0.join("backups/before");
    let said = stdout(&mnemonic(
        &user_home,
        Some(&profile),
        &["backup", into.to_str().unwrap()],
    ));
    assert!(said.contains("checked"), "{said}");
    // Copied as it was, and left as it was: no generation was raised.
    assert_eq!(generation(&into.join("memory.db")), earlier);
    assert_eq!(generation(&store), earlier);
    let kept: i64 = rusqlite::Connection::open(into.join("memory.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, 1);
    // A second backup into the same folder is refused.
    let again = mnemonic(
        &user_home,
        Some(&profile),
        &["backup", into.to_str().unwrap()],
    );
    assert!(!again.status.success());
}

/// Point the profile at a workspace with two projects and at one
/// transcript of the owner in each format.
fn with_history(root: &Path, profile: &Path) {
    let projects = root.join("workspace/projects");
    for folder in ["demoapp", "notes"] {
        std::fs::create_dir_all(projects.join(folder)).unwrap();
    }
    let claude = root.join("claude");
    let codex = root.join("codex");
    std::fs::create_dir_all(claude.join("-code")).unwrap();
    std::fs::create_dir_all(codex.join("2026/09/30")).unwrap();
    let cwd = projects.join("demoapp");
    let record = |minute: u32, role: &str| {
        serde_json::json!({"type": role, "uuid": format!("u{minute}"), "sessionId": "s",
            "cwd": cwd, "entrypoint": "cli",
            "timestamp": format!("2026-09-30T10:{minute:02}:00Z"),
            "message": {"content": "please look at the change"}})
        .to_string()
    };
    std::fs::write(
        claude.join("-code/owner.jsonl"),
        format!("{}\n{}\n", record(0, "user"), record(9, "assistant")),
    )
    .unwrap();
    let rollout = |minute: u32, kind: &str, payload: serde_json::Value| {
        serde_json::json!({"type": kind,
            "timestamp": format!("2026-09-30T11:{minute:02}:00Z"), "payload": payload})
        .to_string()
    };
    std::fs::write(
        codex.join("2026/09/30/rollout-owner.jsonl"),
        format!(
            "{}\n{}\n",
            rollout(
                0,
                "session_meta",
                serde_json::json!({"cwd": projects.join("notes"), "originator": "a desktop client"})
            ),
            rollout(7, "event_msg", serde_json::json!({"type": "task_complete"})),
        ),
    )
    .unwrap();

    let path = profile.join("config.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    for (old, new) in [
        (
            "conversation_enabled = false".to_string(),
            format!(
                "conversation_enabled = true\nconversation_sessions_dir = \"{}\"",
                claude.display()
            ),
        ),
        (
            "codex_enabled = false".to_string(),
            format!(
                "codex_enabled = true\ncodex_sessions_dir = \"{}\"",
                codex.display()
            ),
        ),
        (
            "\n[projects]\n".to_string(),
            format!(
                "\n[projects]\nworkspace = \"{}\"\nfolder = \"projects\"\n",
                root.join("workspace").display()
            ),
        ),
    ] {
        assert_eq!(
            text.matches(&old).count(),
            1,
            "the profile has {old:?} once"
        );
        text = text.replace(&old, &new);
    }
    std::fs::write(&path, text).unwrap();
}

#[test]
fn backfill_cli_reads_history_into_segments_and_a_second_run_reads_nothing() {
    let (root, user_home, profile) = profile("history");
    with_history(&root.0, &profile);
    let first = stdout(&mnemonic(
        &user_home,
        Some(&profile),
        &["attribute", "backfill"],
    ));
    assert!(
        first.contains("2 project(s), 2 transcript(s), 2 with something new"),
        "{first}"
    );
    let store = rusqlite::Connection::open(profile.join("memory.db")).unwrap();
    let segments: Vec<(String, String)> = store
        .prepare(
            "SELECT s.agent, p.slug FROM work_segments s JOIN projects p ON p.id = s.project_id
              ORDER BY 1",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        segments,
        [
            ("claude".to_string(), "demoapp".to_string()),
            ("codex".to_string(), "notes".to_string())
        ]
    );
    let again = stdout(&mnemonic(
        &user_home,
        Some(&profile),
        &["attribute", "backfill"],
    ));
    assert!(again.contains("0 with something new"), "{again}");
    let after: i64 = store
        .query_row("SELECT COUNT(*) FROM work_segments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, 2);
}

/// Every command the first section of the runbook gives is a command of
/// this binary. The runbook is part of the plan of the change, which is
/// not in every checkout: where it is not, there is nothing to hold it to.
#[test]
fn backfill_cli_the_commands_of_the_runbook_exist() {
    let design =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("openspec/changes/live-projects/design.md");
    let Ok(text) = std::fs::read_to_string(design) else {
        return;
    };
    let section = text
        .split("### 20.1")
        .nth(1)
        .expect("the runbook has its first section");
    let section = section.split("\n### ").next().unwrap();
    let root = Temp::new("runbook");
    let mut commands = Vec::new();
    for span in section.split('`').skip(1).step_by(2) {
        let mut words = span.split_whitespace();
        if words.next() != Some("mnemonic") {
            continue;
        }
        // The words that name the command: up to its first argument.
        let named: Vec<&str> = words
            .take_while(|word| word.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
            .take_while(|word| !word.starts_with('-'))
            .collect();
        let mut args = named.clone();
        args.push("--help");
        let out = mnemonic(&root.0, None, &args);
        assert!(out.status.success(), "the runbook gives `{span}`");
        commands.push(named.join(" "));
    }
    for needed in ["backup", "attribute backfill"] {
        assert!(
            commands.iter().any(|command| command == needed),
            "the runbook gives no `mnemonic {needed}`: {commands:?}"
        );
    }
}
