//! Explicit saves and imports through the real binary: what is stored,
//! printed and exported. Credential fixtures are assembled at run time.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn token() -> String {
    let body: String = "a1B2c3D4e5F6".chars().cycle().take(40).collect();
    ["sk-", "proj-", &body].concat()
}

/// A child that sees only a fake home; the model endpoint is a closed
/// local port, so an embedding command falls back to the hash embedder.
fn mnemonic(user_home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
    cmd.env("HOME", user_home)
        .env_remove("MNEMONIC_HOME")
        .env("FASTEMBED_CACHE_DIR", user_home.join(".fastembed_cache"))
        .env("HF_ENDPOINT", "http://127.0.0.1:9")
        .env_remove("HF_HOME");
    for proxy in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
        cmd.env_remove(proxy).env_remove(proxy.to_lowercase());
    }
    cmd.args(args).output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn redaction_memory_cli_save_and_import_store_prepared_text_only() {
    let token = token();
    let root = tempfile::Builder::new()
        .prefix("mn-redact-cli-")
        .tempdir_in("/tmp")
        .unwrap();
    let user_home = root.path().join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.path().join("client");
    let home = profile.to_str().unwrap();
    let init = mnemonic(&user_home, &["--home", home, "init"]);
    assert!(init.status.success(), "{}", text(&init));

    // save: the printed title and the stored row are prepared.
    let saved = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "save",
            "--title",
            &format!("deploy key {token}"),
            &format!("не так, the key is {token} now"),
        ],
    );
    let out = text(&saved);
    assert!(saved.status.success());
    assert!(!out.contains(&token), "the CLI printed the raw title");
    assert!(out.contains("Redacted:") && out.contains("Saved:"));

    // tags are redacted as one string before they are split.
    let tagged = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "save",
            "--title",
            "tagged",
            "--tags",
            "public,<private>one,two</private>",
            "a plain note about tags",
        ],
    );
    assert!(tagged.status.success(), "{}", text(&tagged));

    // A project name is judged raw, before resolution to an existing
    // project entity (extraction creates them as `password-<v>`) could drop
    // the syntax that made it a credential.
    let value: String = "a1b2c3d4e5f6".chars().cycle().take(32).collect();
    {
        use mnemonic_agent::graph::{Entity, EntityType};
        use mnemonic_agent::storage::Storage;
        let storage = Storage::open(&profile.join("memory.db")).unwrap();
        storage
            .upsert_entity(&Entity {
                name: format!("password-{value}"),
                entity_type: EntityType::Project,
            })
            .unwrap();
    }
    // The seed resolves names: a spelling that differs by case lands on it.
    let resolved = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "save",
            "--title",
            "seed",
            "--project",
            &format!("PASSWORD-{value}"),
            "a plain note for the project",
        ],
    );
    assert!(resolved.status.success(), "{}", text(&resolved));
    let refused = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "save",
            "--title",
            "t3",
            "--project",
            &format!("password={value}"),
            "another plain note here",
        ],
    );
    let out = text(&refused);
    assert!(!refused.status.success() && out.contains("SENSITIVE_CONTENT"));

    // save --project with a credential is refused before anything is written.
    let refused = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "save",
            "--title",
            "t",
            "--project",
            &format!("p-{token}"),
            "plain content here",
        ],
    );
    let out = text(&refused);
    assert!(!refused.status.success());
    assert!(out.contains("SENSITIVE_CONTENT") && !out.contains(&token));

    // import: records are prepared; a refused one stops the whole file.
    let file = root.path().join("import.json");
    std::fs::write(
        &file,
        serde_json::json!([{
            "id": "imp-1", "timestamp": "2026-01-02T03:04:05Z", "title": "imported",
            "content": format!("token = {token}"), "memory_type": "note",
            "tags": "[\"a\"]", "source": "\"manual\"", "importance": 0.5, "metadata": "{}"
        }])
        .to_string(),
    )
    .unwrap();
    let imported = mnemonic(
        &user_home,
        &["--home", home, "import", file.to_str().unwrap()],
    );
    let out = text(&imported);
    assert!(imported.status.success() && out.contains("Imported: 1"));
    std::fs::write(
        &file,
        serde_json::json!([{
            "id": format!("imp-{token}"), "timestamp": "2026-01-02T03:04:05Z", "title": "t",
            "content": "c", "memory_type": "note", "tags": "[]", "source": "\"manual\"",
            "importance": 0.5, "metadata": "{}"
        }])
        .to_string(),
    )
    .unwrap();
    let blocked = mnemonic(
        &user_home,
        &["--home", home, "import", file.to_str().unwrap()],
    );
    let out = text(&blocked);
    assert!(!blocked.status.success());
    assert!(out.contains("record 0") && !out.contains(&token));

    // export shows only prepared text.
    let export = mnemonic(&user_home, &["--home", home, "export"]);
    let out = text(&export);
    assert!(export.status.success());
    assert!(!out.contains(&token), "the export holds the credential");
    assert!(out.matches("[REDACTED:credential]").count() >= 3);
    assert!(!out.contains("two") && out.contains("[REDACTED:private]"));
    assert!(
        out.contains(&format!("password-{value}")) && !out.contains("PASSWORD-"),
        "the project did not resolve to the seeded entity"
    );
}

/// Explicit state from the command line: what names a record is refused
/// with a fixed code and no echo, narrative is stored prepared.
#[test]
fn redaction_state_cli_refuses_identities_and_prepares_narrative() {
    let token = token();
    let value: String = "a1B2c3D4e5F6".chars().cycle().take(24).collect();
    let root = tempfile::Builder::new()
        .prefix("mn-redact-state-cli-")
        .tempdir_in("/tmp")
        .unwrap();
    let user_home = root.path().join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.path().join("client");
    let home = profile.to_str().unwrap();
    let init = mnemonic(&user_home, &["--home", home, "init"]);
    assert!(init.status.success(), "{}", text(&init));
    let run = |args: &[&str]| {
        let mut all = vec!["--home", home];
        all.extend_from_slice(args);
        let out = mnemonic(&user_home, &all);
        (out.status.success(), text(&out))
    };

    let project = format!("p-{token}");
    let refused: [&[&str]; 17] = [
        &["fact", "set", "api", "key", &token],
        &["fact", "set", "api", "password", &value],
        &[
            "fact",
            "set",
            "api",
            "password",
            &value,
            "--qualifier",
            "staging",
        ],
        &["fact", "set", &token, "host", "db.internal"],
        // Clean as written, filed under the key `api-key`.
        &["fact", "set", "stripe", "api key", &value],
        &[
            "fact",
            "set",
            "svc",
            "build id",
            &value,
            "--project",
            "token",
        ],
        &[
            "fact",
            "set",
            "api",
            "host",
            "db.internal",
            "--project",
            &project,
        ],
        &[
            "fact",
            "set",
            "api",
            "host",
            "db.internal",
            "--source",
            &token,
        ],
        &["fact", "forget", &token, "--yes"],
        &["followup", "add", "--project", &project, "plain"],
        &["followup", "close", &token],
        &["peer", "add", &token],
        &["peer", "merge", "user", &token],
        &["conclusion", "add", &token, "plain"],
        &["conclusion", "generate", &token],
        &["conclusion", "delete", &token],
        &["conclusion", "supersede", &token, "0123456789abcdef"],
    ];
    for args in refused {
        let (ok, out) = run(args);
        assert!(!ok, "admitted: {:?}", &args[..2]);
        assert!(out.contains("SENSITIVE_CONTENT"), "{:?}", &args[..2]);
        assert!(
            !out.contains(&token) && !out.contains(&value),
            "echoed: {:?}",
            &args[..2]
        );
    }

    let (ok, _) = run(&["fact", "set", "api", "host", "db.internal"]);
    assert!(ok);
    // A fact about a credential may state when it expires.
    let (ok, _) = run(&[
        "fact",
        "set",
        "GITHUB_TOKEN",
        "expires",
        "2026-12-01T00:00:00Z",
    ]);
    assert!(ok);
    let (ok, out) = run(&[
        "followup",
        "add",
        "--project",
        "demoapp",
        &format!("rotate {token} before friday"),
    ]);
    assert!(ok && out.contains("[REDACTED:credential]"));
    assert!(!out.contains(&token));
    let (ok, _) = run(&[
        "conclusion",
        "add",
        "user",
        &format!("keeps {token} in the shell profile"),
    ]);
    assert!(ok);

    // What the store shows afterwards.
    let mut shown = String::new();
    for args in [
        &["fact", "current", "api", "--history"][..],
        &["followup", "list", "--all"][..],
        &["conclusion", "list", "user"][..],
        &["peer", "list"][..],
    ] {
        let (ok, out) = run(args);
        assert!(ok);
        shown.push_str(&out);
    }
    assert!(!shown.contains(&token) && !shown.contains(&value));
    assert!(shown.contains("db.internal"));
    assert!(shown.matches("[REDACTED:credential]").count() == 2);
}

/// Text given on the command line to be searched by or to head a context
/// is new text: it is embedded and shown prepared. A path names a file,
/// and one the policy refuses is neither written nor shown.
#[test]
fn redaction_generated_cli_prepares_queries_and_topics_and_refuses_a_path() {
    let token = token();
    let root = tempfile::Builder::new()
        .prefix("mn-redact-gen-")
        .tempdir_in("/tmp")
        .unwrap();
    let user_home = root.path().join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.path().join("client");
    let home = profile.to_str().unwrap();
    let init = mnemonic(&user_home, &["--home", home, "init"]);
    assert!(init.status.success(), "{}", text(&init));

    let similar = mnemonic(
        &user_home,
        &["--home", home, "similar", &format!("where is {token} kept")],
    );
    let out = text(&similar);
    assert!(similar.status.success());
    assert!(!out.contains(&token), "the query is shown as it was given");
    assert!(out.contains("No similar memories found for: where is [REDACTED:credential] kept"));

    let output = root.path().join("CONTEXT.md");
    let context = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "context",
            "--topic",
            &format!("deploys with {token}"),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    let out = text(&context);
    assert!(context.status.success());
    assert!(!out.contains(&token), "the topic is shown as it was given");
    assert!(out.contains("\"deploys with [REDACTED:credential]\""));

    let refused = root.path().join(format!("{token}.md"));
    let context = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "context",
            "--output",
            refused.to_str().unwrap(),
        ],
    );
    let out = text(&context);
    assert!(!context.status.success());
    assert!(out.contains("SENSITIVE_CONTENT"));
    assert!(!out.contains(&token), "the path is said back");
    assert!(!refused.exists());
}

/// Every file of a directory and below it, as (path, bytes).
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((path.to_string_lossy().into_owned(), bytes));
            }
        }
    }
    out
}

/// After a save through the binary, with the sinks a fresh profile has,
/// no file of the profile or of the home holds what was redacted: not the
/// store, not its journal, not what a sink wrote, and no file's name.
#[test]
fn redaction_outputs_cli_no_file_holds_what_a_save_redacted() {
    let token = token();
    let root = tempfile::Builder::new()
        .prefix("mn-redact-out-")
        .tempdir_in("/tmp")
        .unwrap();
    let user_home = root.path().join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.path().join("client");
    let home = profile.to_str().unwrap();
    let init = mnemonic(&user_home, &["--home", home, "init"]);
    assert!(init.status.success(), "{}", text(&init));

    let saved = mnemonic(
        &user_home,
        &[
            "--home",
            home,
            "save",
            "--title",
            &format!("deploy key {token}"),
            "--tags",
            &format!("deploy,{token}"),
            &format!("we keep {token} <private>and a plan of ours</private> here"),
        ],
    );
    let out = text(&saved);
    assert!(saved.status.success());
    assert!(!out.contains(&token) && !out.contains("plan of ours"));

    let files = files_under(root.path());
    assert!(files.iter().any(|(path, _)| path.ends_with("memory.db")));
    for (path, bytes) in &files {
        let holds = |text: &str| bytes.windows(text.len()).any(|w| w == text.as_bytes());
        assert!(!path.contains(&token), "a file is named after the token");
        assert!(!holds(&token), "a file holds the token");
        assert!(!holds("plan of ours"), "a file holds the private text");
    }
    let marked = files
        .iter()
        .filter(|(_, bytes)| {
            let marker = b"[REDACTED:credential]";
            bytes.windows(marker.len()).any(|w| w == marker)
        })
        .count();
    assert!(marked >= 1, "nothing was stored");
}

/// What a command says of what it did not find, or failed with, is said
/// without what the policy refuses: a command line can hold it, and an
/// error can say it back.
#[test]
fn redaction_outputs_cli_does_not_say_back_what_the_policy_refuses() {
    let token = token();
    let root = tempfile::Builder::new()
        .prefix("mn-redact-err-")
        .tempdir_in("/tmp")
        .unwrap();
    let user_home = root.path().join("user");
    std::fs::create_dir_all(&user_home).unwrap();
    let profile = root.path().join("client");
    let home = profile.to_str().unwrap();
    let init = mnemonic(&user_home, &["--home", home, "init"]);
    assert!(init.status.success(), "{}", text(&init));

    let asked = format!("where is {token} kept");
    // A name of the graph from before the policy.
    {
        drop(mnemonic_agent::storage::Storage::open(&profile.join("memory.db")).unwrap());
        let conn = rusqlite::Connection::open(profile.join("memory.db")).unwrap();
        conn.execute(
            "INSERT INTO entity_aliases (alias, canonical, merged_at)
             VALUES (?1, 'demoapp', '2026-01-02T03:04:05Z')",
            [token.to_lowercase()],
        )
        .unwrap();
    }
    // A peer from before the policy, with no session.
    let other: String = ["sk-", "proj-", &"z9Y8x7W6v5U4".repeat(4)].concat();
    {
        let conn = rusqlite::Connection::open(profile.join("memory.db")).unwrap();
        conn.execute(
            "INSERT INTO peers (id, name, kind, created_at, last_seen_at)
             VALUES ('p-1', ?1, 'agent', '2026-01-02T03:04:05Z', '2026-01-02T03:04:05Z')",
            [other.to_lowercase()],
        )
        .unwrap();
    }
    let alias = token.to_lowercase();
    let lines: [(&[&str], &str); 12] = [
        (&["query", &asked], "No results for: where is"),
        (&["forget", &token], "No memory with id"),
        (&["graph", &token], "not found in graph"),
        (&["session", "show", &token], "No session with id"),
        (&["graph", &alias], "is an alias of 'demoapp'"),
        (&["peer", "sessions", &token], "No peer named"),
        (&["conclusion", "list", &token], "conclusions for"),
        (&["session", "list", "--peer", &token], "No peer named"),
        (&["session", "list", "--peer", &other], "No sessions found"),
        (
            &["fact", "current", "--project", "demo", &token],
            "No facts about",
        ),
        (
            &["conclusion", "supersede", &token, &token],
            "SENSITIVE_CONTENT",
        ),
        (&["conclusion", "generate", &token], "SENSITIVE_CONTENT"),
    ];
    for (line, says) in lines {
        let mut args = vec!["--home", home];
        args.extend_from_slice(line);
        let out = text(&mnemonic(&user_home, &args));
        assert!(!out.contains(&token), "a command says its argument back");
        // Some say a name in lower case.
        assert!(
            !out.contains(&token.to_lowercase()) && !out.contains(&other.to_lowercase()),
            "a command says its argument back in lower case"
        );
        assert!(out.contains(says), "a command does not say what it should");
    }
    // An error that quotes what it read: a parser shows the line it
    // stopped at.
    let config = profile.join("config.toml");
    let kept = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("[output]\nmemory_api_url = \"{token}\n")).unwrap();
    let failed = mnemonic(&user_home, &["--home", home, "recent"]);
    let out = text(&failed);
    assert!(failed.status.code() == Some(1) && out.contains("Error: "));
    assert!(
        out.contains("memory_api_url"),
        "the error does not show the line"
    );
    assert!(!out.contains(&token), "an error quotes what it read");
    std::fs::write(&config, kept).unwrap();

    // What the policy admits is said as it was given.
    let out = text(&mnemonic(&user_home, &["--home", home, "forget", "m-1"]));
    assert!(out.contains("No memory with id `m-1`"));
    let out = text(&mnemonic(
        &user_home,
        &["--home", home, "query", "rollout day"],
    ));
    assert!(out.contains("No results for: rollout day"));
}
