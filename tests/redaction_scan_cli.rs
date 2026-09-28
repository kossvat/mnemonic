//! The scanner through the real binary: its exit codes, what it prints,
//! and that it leaves no file anywhere. Credential fixtures are assembled
//! at run time.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn token() -> String {
    let body: String = "a1B2c3D4e5F6".chars().cycle().take(40).collect();
    ["sk-", "proj-", &body].concat()
}

/// A child that sees only a fake home. The scanner loads no model, so
/// nothing is set up for one: a scan that reached for it would show in
/// the home.
fn mnemonic(user_home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
    cmd.env("HOME", user_home)
        .env_remove("MNEMONIC_HOME")
        .env_remove("FASTEMBED_CACHE_DIR")
        .env_remove("HF_HOME");
    cmd.args(args).output().unwrap()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A closed snapshot of a private store, alone in `dir`, with one memory
/// whose text is `content`, stored as a build before the policy did.
fn snapshot(root: &Path, name: &str, content: &str) -> PathBuf {
    let live = root.join(format!("{name}-live"));
    std::fs::create_dir(&live).unwrap();
    let db = live.join("memory.db");
    drop(mnemonic_agent::storage::Storage::open(&db).unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source,
            importance, metadata)
         VALUES ('m-1', '2026-01-02T03:04:05Z', 'Rollout', ?1, 'note', '[]', '\"Manual\"',
            0.5, 'null')",
        [content],
    )
    .unwrap();
    let dir = root.join(name);
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("snapshot.db");
    conn.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    path
}

#[test]
fn redaction_scan_cli_exit_codes_tell_clean_from_found_from_unread() {
    let token = token();
    let root = tempfile::Builder::new()
        .prefix("mn-scan-cli-")
        .tempdir_in("/tmp")
        .unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let profile = root.path().join("profile");
    let clean = snapshot(root.path(), "clean", "friday");
    let dirty = snapshot(root.path(), "dirty", &format!("the key is {token}"));
    let missing = root.path().join(format!("{token}.db"));
    let open = snapshot(root.path(), "open", "friday");
    std::fs::write(open.with_file_name("snapshot.db-wal"), "").unwrap();

    let cases: [(&Path, i32, bool); 4] = [
        (&clean, 0, true),
        (&dirty, 2, true),
        (&missing, 1, false),
        (&open, 1, false),
    ];
    for (path, code, complete) in cases {
        let dir = path.parent().unwrap();
        let before = (std::fs::read(path).ok(), listing(dir));
        let out = mnemonic(
            &home,
            &[
                "--home",
                profile.to_str().unwrap(),
                "redact",
                "scan",
                "--db",
                path.to_str().unwrap(),
            ],
        );
        assert!(out.status.code() == Some(code));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(report["complete"] == complete);
        assert!(report["policy_version"] == 1);
        let said = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
        assert!(!said.contains(&token), "the scanner said what it read");
        assert!(!said.contains(root.path().to_str().unwrap()));
        let after = (std::fs::read(path).ok(), listing(dir));
        assert!(before == after, "the scan changed the snapshot");
    }

    // No profile, no configuration, no log, no model cache: the scanner
    // made nothing of its own.
    assert!(listing(&home).is_empty(), "the scanner wrote to the home");
    assert!(!profile.exists(), "the scanner made a profile");
}

/// A command line the scanner cannot read is bad input, by the scanner's
/// code for it, and is not said back: what was given is a path.
#[test]
fn redaction_scan_cli_bad_command_lines_are_bad_input_and_are_not_echoed() {
    let token = token();
    let root = tempfile::Builder::new()
        .prefix("mn-scan-cli-")
        .tempdir_in("/tmp")
        .unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let clean = snapshot(root.path(), "clean", "friday");
    let clean = clean.to_str().unwrap();
    let stray = format!("/tmp/{token}.db");
    let lines: [&[&str]; 5] = [
        &["redact", "scan"],
        &["redact", "scan", &stray],
        &["redact", "scan", "--db", clean, "--apply"],
        &["redact", "scan", "--db", clean, &format!("--{token}")],
        &["redact", "scan", "--db", clean, "--format", "yaml"],
    ];
    for line in lines {
        let out = mnemonic(&home, line);
        assert!(out.status.code() == Some(1));
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!said.contains(&token), "the command line is said back");
        assert!(
            said.contains("usage: mnemonic redact scan") || said.contains("UNSUPPORTED_FORMAT")
        );
    }
    // A home whose name is no text, before a stray argument.
    use std::os::unix::ffi::OsStrExt;
    let out = Command::new(env!("CARGO_BIN_EXE_mnemonic"))
        .env("HOME", &home)
        .env_remove("MNEMONIC_HOME")
        .arg(std::ffi::OsStr::from_bytes(b"--home=/tmp/mn-no-text-\xff"))
        .args(["redact", "scan", &stray])
        .output()
        .unwrap();
    assert!(out.status.code() == Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("usage: mnemonic redact scan") && !said.contains(&token));

    // Asking for help is not an error.
    let out = mnemonic(&home, &["redact", "scan", "--help"]);
    assert!(out.status.code() == Some(0));
    assert!(listing(&home).is_empty());
}
