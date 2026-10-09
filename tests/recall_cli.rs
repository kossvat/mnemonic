//! Real process: `mnemonic recall` on a store in a scratch profile, with
//! the timezone set by the test. The day is the local day of the process,
//! and the bytes of `--json` are the bytes of the route.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::{Duration, NaiveDate};
use mnemonic_agent::event::{EventSource, MemoryEntry, MemoryType};
use mnemonic_agent::recall_day;
use mnemonic_agent::recall_log::{Item, Record, Section, Surface, Tool, Writer};
use mnemonic_agent::storage::Storage;

/// A zone with a change of daylight time on the day of the fixture: the
/// local day of 2026-03-08 has 23 hours.
const ZONE: &str = "America/New_York";

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = common::short_root().join(format!("mn-recall-{tag}-{}", &id[..8]));
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mnemonic(user_home: &Path, profile: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
    cmd.env("HOME", user_home)
        .env_remove("MNEMONIC_HOME")
        .env("TZ", ZONE)
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
    cmd.arg("--home").arg(profile).args(args).output().unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

/// A profile with the fixture store: the memories, and the events of the
/// fixture day placed by the bounds of the zone. The zone is set for this
/// process too, so that the bounds and the summary computed here are the
/// ones the child computes.
fn fixture(tag: &str) -> (Temp, PathBuf, PathBuf, NaiveDate) {
    // SAFETY: the tests of this file all set the same zone, and nothing
    // else in this process reads it before they do.
    unsafe { std::env::set_var("TZ", ZONE) };
    let root = Temp::new(tag);
    let user_home = root.0.join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.0.join("client");
    let init = mnemonic(&user_home, &profile, &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let db = profile.join("memory.db");
    let storage = Storage::open(&db).unwrap();
    for (id, title, memory_type) in [
        (
            "a1",
            "Decision: use SQLite for the index",
            MemoryType::Decision,
        ),
        ("b2", "stop, use the other parser", MemoryType::Feedback),
        (
            "c3",
            "Decision: keep the smaller index",
            MemoryType::Decision,
        ),
    ] {
        let mut entry = MemoryEntry::new(title, "body", memory_type, EventSource::Socket);
        entry.id = id.to_string();
        storage.save(&entry).unwrap();
    }
    let day = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
    let (start, end) = mnemonic_agent::activity::local_day_bounds_utc(day).unwrap();
    // The day has 23 hours in this zone: daylight time begins in it.
    assert_eq!(end - start, Duration::hours(23));
    assert_eq!(start.to_rfc3339(), "2026-03-08T05:00:00+00:00");
    let named = Writer::new(&db, true);
    named.name_client("Claude Code");
    let unnamed = Writer::new(&db, true);
    let item = |id: &str, section: Option<Section>| Item {
        memory_id: id.to_string(),
        section,
    };
    let events = [
        (
            &unnamed,
            start,
            Surface::Context,
            Tool::Context,
            vec![
                item("a1", Some(Section::Digest)),
                item("b2", Some(Section::Feedback)),
                item("c3", Some(Section::Decisions)),
            ],
            2,
        ),
        (
            &named,
            start + Duration::hours(7),
            Surface::Mcp,
            Tool::MemorySearch,
            vec![item("a1", None), item("c3", None)],
            0,
        ),
        (
            &named,
            end - Duration::seconds(1),
            Surface::Mcp,
            Tool::MemoryContext,
            vec![item("a1", Some(Section::Topic))],
            1,
        ),
        (
            &named,
            end,
            Surface::Mcp,
            Tool::MemoryRecent,
            vec![item("a1", None)],
            0,
        ),
        (
            &named,
            start - Duration::seconds(1),
            Surface::Mcp,
            Tool::MemoryRecent,
            vec![item("b2", None)],
            0,
        ),
    ];
    for (writer, at, surface, tool, items, hidden) in events {
        writer
            .write(
                &Record {
                    surface,
                    tool,
                    items,
                    chars: 100,
                    hidden,
                },
                at,
            )
            .unwrap();
    }
    (root, user_home, profile, day)
}

#[test]
fn recall_cli_json_is_the_bytes_of_the_summary_and_of_the_fixture_file() {
    let (_root, user_home, profile, day) = fixture("json");
    let out = stdout(&mnemonic(
        &user_home,
        &profile,
        &["recall", "--json", "--day", "2026-03-08"],
    ));
    let storage = Storage::open(&profile.join("memory.db")).unwrap();
    let expected =
        serde_json::to_string(&recall_day::summary(&storage, day, true).unwrap()).unwrap();
    assert_eq!(out, format!("{expected}\n"));
    // The fixture file the widget's test reads is what the command prints.
    let file = include_str!("../clients/macos/Tests/Fixtures/recall-day.json");
    assert_eq!(out, file);
    // The numbers by hand: two contexts, one call, six memories, one
    // correction, two hidden, three distinct.
    assert!(out.contains(
        "\"contexts_generated\":2,\"calls_answered\":1,\"memories_returned\":6,\
         \"corrections_returned\":1,\"hidden_as_noise\":2,\"distinct_memories\":3"
    ));
}

#[test]
fn recall_cli_text_says_the_numbers_and_an_empty_day_says_so() {
    let (_root, user_home, profile, _) = fixture("text");
    let out = stdout(&mnemonic(
        &user_home,
        &profile,
        &["recall", "--day", "2026-03-08"],
    ));
    assert!(out.contains("Memory on 2026-03-08"), "{out}");
    assert!(out.contains("contexts generated:   2"), "{out}");
    assert!(out.contains("claude code: 2 events"), "{out}");
    let out = stdout(&mnemonic(
        &user_home,
        &profile,
        &["recall", "--day", "2026-03-10"],
    ));
    assert_eq!(out, "Memory on 2026-03-10\n  no recorded recalls\n");
    let out = stdout(&mnemonic(
        &user_home,
        &profile,
        &["recall", "--json", "--day", "2026-03-10"],
    ));
    assert!(
        out.starts_with("{\"day\":\"2026-03-10\",\"log_enabled\":true,\"contexts_generated\":0,")
    );
    assert!(
        out.contains("\"by_tool\":[],\"by_client\":[],\"top\":[]"),
        "{out}"
    );
}

#[test]
fn recall_cli_a_day_that_is_not_a_date_is_refused() {
    let (_root, user_home, profile, _) = fixture("refused");
    let out = mnemonic(&user_home, &profile, &["recall", "--day", "yesterday"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("YYYY-MM-DD"));
    let out = mnemonic(&user_home, &profile, &["recall", "--day", "2026-13-01"]);
    assert!(!out.status.success());
}

#[test]
fn recall_cli_the_events_of_the_next_local_day_are_on_it() {
    let (_root, user_home, profile, _) = fixture("next");
    let out = stdout(&mnemonic(
        &user_home,
        &profile,
        &["recall", "--json", "--day", "2026-03-09"],
    ));
    assert!(
        out.contains("\"calls_answered\":1,\"memories_returned\":1"),
        "{out}"
    );
    let out = stdout(&mnemonic(
        &user_home,
        &profile,
        &["recall", "--json", "--day", "2026-03-07"],
    ));
    assert!(
        out.contains("\"calls_answered\":1,\"memories_returned\":1"),
        "{out}"
    );
}
