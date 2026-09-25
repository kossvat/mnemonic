//! Real process round trip: no running daemon, private config, model or network.
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("mnemonic-shared-cli-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn command(db: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
    cmd.args(["shared", "--db"]).arg(db);
    cmd
}

#[test]
fn actual_cli_mcp_exchange_shares_context_but_quarantines_agent_writes() {
    let temp = Temp::new();
    let db = temp.0.join("shared.db");
    let source = temp.0.join("brief.txt");
    std::fs::write(&source, "Reviewed project brief").unwrap();
    let publish = command(&db)
        .args([
            "publish",
            "--project",
            "alpha",
            "--key",
            "brief",
            "--title",
            "Brief",
            "--source",
            "git:example@abc:brief",
        ])
        .arg("--file")
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        publish.status.success(),
        "{}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let policy = temp.0.join("policy.toml");
    std::fs::write(
        &policy,
        "version=1\nproject_id='alpha'\nagent_id='researcher'\nallow_observations=true\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut child = command(&db)
        .arg("serve")
        .arg("--policy")
        .arg(&policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shared_context","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shared_observe","arguments":{
            "request_id":"job-1","title":"Observation","content":"UNTRUSTED_INPUT","source":"https://example.test/lead"
        }}}),
    ];
    {
        let mut input = child.stdin.take().unwrap();
        for frame in frames {
            writeln!(input, "{frame}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    assert!(responses[1].to_string().contains("Reviewed project brief"));
    assert_eq!(responses[2]["result"]["isError"], false);
    let context = command(&db)
        .args(["context", "--project", "alpha"])
        .output()
        .unwrap();
    assert!(context.status.success());
    assert!(!String::from_utf8_lossy(&context.stdout).contains("UNTRUSTED_INPUT"));
    let inbox = command(&db)
        .args(["inbox", "--project", "alpha"])
        .output()
        .unwrap();
    assert!(inbox.status.success());
    let inbox: Value = serde_json::from_slice(&inbox.stdout).unwrap();
    assert_eq!(inbox["observations"][0]["writer_id"], "researcher");
    assert_eq!(inbox["observations"][0]["status"], "pending");
    let revoke = command(&db)
        .args(["revoke", "--project", "alpha", "--key", "brief"])
        .output()
        .unwrap();
    assert!(revoke.status.success());
    let context = command(&db)
        .args(["context", "--project", "alpha"])
        .output()
        .unwrap();
    let context: Value = serde_json::from_slice(&context.stdout).unwrap();
    assert!(context["records"].as_array().unwrap().is_empty());
    assert!(context["revision"].as_i64().unwrap() > 1);
}

#[test]
fn cli_requires_separate_explicit_store_and_preserves_private_database() {
    let help = Command::new(env!("CARGO_BIN_EXE_mnemonic"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let missing = Command::new(env!("CARGO_BIN_EXE_mnemonic"))
        .args(["shared", "context", "--project", "alpha"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let temp = Temp::new();
    let private = temp.0.join("private.db");
    {
        let conn = rusqlite::Connection::open(&private).unwrap();
        conn.execute_batch("CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT); INSERT INTO memories VALUES ('secret','PRIVATE_CONTEXT');").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let result = command(&private)
        .args(["context", "--project", "alpha"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("PRIVATE_CONTEXT"));
    let conn = rusqlite::Connection::open(&private).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='shared_records'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let body: String = conn
        .query_row("SELECT content FROM memories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(body, "PRIVATE_CONTEXT");
}

#[cfg(unix)]
#[test]
fn cli_rejects_unsafe_database_and_policy_directories_before_creating_database() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let unsafe_dir = temp.0.join("writable");
    std::fs::create_dir(&unsafe_dir).unwrap();
    std::fs::set_permissions(&unsafe_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let unsafe_db = unsafe_dir.join("shared.db");
    let result = command(&unsafe_db)
        .args(["context", "--project", "alpha"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!unsafe_db.exists());
    let policy = unsafe_dir.join("policy.toml");
    std::fs::write(&policy, "version=1\nproject_id='alpha'\nagent_id='scout'\n").unwrap();
    std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o600)).unwrap();
    let safe_db = temp.0.join("shared.db");
    let result = command(&safe_db)
        .args(["serve", "--policy"])
        .arg(&policy)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!safe_db.exists());
}

#[test]
fn malformed_tool_arguments_cannot_inject_content_into_transport_logs() {
    let temp = Temp::new();
    let db = temp.0.join("shared.db");
    let policy = temp.0.join("policy.toml");
    std::fs::write(
        &policy,
        "version=1\nproject_id='alpha'\nagent_id='researcher'\n",
    )
    .unwrap();
    let mut child = command(&db)
        .args(["serve", "--policy"])
        .arg(&policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shared_context","arguments":{"limit":"SYNTHETIC_SECRET"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shared_context","arguments":{"x\nFORGED_EVENT\u{001b}[2J":1}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
    ];
    {
        let mut input = child.stdin.take().unwrap();
        for frame in frames {
            writeln!(input, "{frame}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    for stream in [&stdout, &stderr] {
        assert!(!stream.contains("SYNTHETIC_SECRET"));
        assert!(!stream.contains("FORGED_EVENT"));
        assert!(!stream.contains('\u{001b}'));
    }
    let responses: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[1]["result"]["isError"], true);
    assert_eq!(responses[2]["result"]["isError"], true);
    assert_eq!(responses[3]["id"], 4);
    assert_eq!(responses[3]["result"], json!({}));
    assert_eq!(stderr.lines().count(), 2);
}

/// One project, two people, each with a reading agent and a writing identity:
/// the whole path from a draft to shared truth, through the real binary.
#[test]
#[cfg(unix)]
fn two_people_share_one_project_without_reaching_each_other() {
    use std::os::unix::fs::PermissionsExt;

    let temp = Temp::new();
    let db = temp.0.join("shared.db");
    let policies = temp.0.join("policies");
    std::fs::create_dir(&policies).unwrap();
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_mnemonic"));

    let run = |args: &[&str]| {
        let output = command(&db).args(args).output().unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )
    };
    let json = |args: &[&str]| -> Value {
        let (ok, stdout) = run(args);
        assert!(ok, "{args:?} failed");
        serde_json::from_str(&stdout).unwrap()
    };

    // The database serves exactly this project.
    assert_eq!(
        json(&["pin", "--project", "demoapp"])["pinned_project"],
        "demoapp"
    );

    // Four keys: a reader and a writer for each person, one key per agent.
    // sshd matches on the key, so a shared key would silently give both of a
    // person's agents whichever policy is listed first.
    let mut keys = String::new();
    let mut seed = 1u8;
    for (person, agent, write) in [
        ("ann", "claude", false),
        ("ann", "contrib", true),
        ("ben", "claude", false),
        ("ben", "contrib", true),
    ] {
        let public_key = temp.0.join(format!("{person}-{agent}.pub"));
        std::fs::write(&public_key, ed25519_key(seed, person)).unwrap();
        seed += 1;
        let mut args: Vec<String> = vec![
            "keys".into(),
            "grant".into(),
            "--project".into(),
            "demoapp".into(),
            "--principal".into(),
            person.into(),
            "--agent".into(),
            agent.into(),
            "--bin".into(),
            bin.display().to_string(),
            "--policy-dir".into(),
            policies.display().to_string(),
            "--public-key".into(),
            public_key.display().to_string(),
        ];
        if write {
            args.push("--write".into());
        }
        let grant = json(&args.iter().map(String::as_str).collect::<Vec<_>>());
        let policy_path = PathBuf::from(grant["policy_path"].as_str().unwrap());
        std::fs::write(&policy_path, grant["policy_toml"].as_str().unwrap()).unwrap();
        std::fs::set_permissions(&policy_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        keys.push_str(grant["authorized_keys_line"].as_str().unwrap());
        keys.push('\n');
    }
    let authorized_keys = temp.0.join("authorized_keys");
    std::fs::write(&authorized_keys, &keys).unwrap();
    let lint: Vec<String> = vec![
        "keys".into(),
        "lint".into(),
        "--authorized-keys".into(),
        authorized_keys.display().to_string(),
        "--bin".into(),
        bin.display().to_string(),
        "--policy-dir".into(),
        policies.display().to_string(),
    ];
    let lint: Vec<&str> = lint.iter().map(String::as_str).collect();
    assert_eq!(json(&lint)["problems"].as_array().unwrap().len(), 0);

    // Every way a hand-edited line hands back more than `serve` exits non-zero.
    for (what, edited) in [
        (
            "the forced command removed",
            keys.replace("restrict,command=", "x-removed="),
        ),
        ("restrict dropped", keys.replacen("restrict,", "", 1)),
        (
            "a capability added back",
            keys.replacen("restrict,", "restrict,pty,", 1),
        ),
        (
            "another binary in the command",
            keys.replacen(bin.to_str().unwrap(), "/bin/sh", 1),
        ),
    ] {
        let broken = temp.0.join("broken_keys");
        std::fs::write(&broken, &edited).unwrap();
        let mut broken_lint = lint.clone();
        broken_lint[3] = broken.to_str().unwrap();
        assert!(!run(&broken_lint).0, "{what} must fail the lint");
    }

    // Ben's agent files a draft; it stays invisible until Ann promotes it.
    let ben_policy = policies.join("ben-contrib.toml");
    let mut writer = command(&db)
        .args(["serve", "--policy"])
        .arg(&ben_policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let observe = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shared_observe",
        "arguments":{"request_id":"r1","title":"Pricing","content":"The Wombat plan costs 42",
        "source":"call:2026-09-20"}}});
    writer
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            format!(
                "{}\n{}\n{observe}\n",
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .as_bytes(),
        )
        .unwrap();
    drop(writer.stdin.take());
    let receipt = writer.wait_with_output().unwrap();
    let receipt = String::from_utf8_lossy(&receipt.stdout);
    assert!(receipt.contains("untrusted_observation"), "{receipt}");
    assert!(
        !receipt.contains("Wombat"),
        "the reply must not echo the draft"
    );

    // A reader of either person sees nothing of it.
    let read = |agent: &str, tool: &str| -> String {
        let policy = policies.join(format!("{agent}.toml"));
        let mut server = command(&db)
            .args(["serve", "--policy"])
            .arg(&policy)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        server
            .stdin
            .as_mut()
            .unwrap()
            .write_all(
                format!(
                    "{}\n{}\n{}\n",
                    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
                    json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":{}}})
                )
                .as_bytes(),
            )
            .unwrap();
        drop(server.stdin.take());
        String::from_utf8_lossy(&server.wait_with_output().unwrap().stdout).into_owned()
    };
    for agent in ["ann-claude", "ben-claude"] {
        assert!(!read(agent, "shared_context").contains("Wombat"), "{agent}");
    }

    // Ann reads the inbox and promotes that one draft.
    let inbox = json(&["inbox", "--project", "demoapp"]);
    assert_eq!(inbox["trust"], "untrusted_observations");
    let observation = &inbox["observations"][0];
    assert_eq!(observation["principal_id"], "ben");
    let id = observation["id"].as_str().unwrap();
    let record = json(&[
        "promote",
        "--project",
        "demoapp",
        "--id",
        id,
        "--key",
        "pricing",
        "--expect-absent",
        "--actor",
        "ann",
    ]);
    assert_eq!(record["published_by"], "ann");
    assert_eq!(record["origin_writer_id"], "ben-contrib");

    // Now both people's agents see it.
    for agent in ["ann-claude", "ben-claude"] {
        assert!(read(agent, "shared_context").contains("Wombat"), "{agent}");
    }

    // Ann cuts Ben off; Ben's agents stop, Ann's keep working.
    assert_eq!(
        json(&[
            "access",
            "revoke",
            "--project",
            "demoapp",
            "--principal",
            "ben",
            "--actor",
            "ann"
        ])["revoked"],
        true
    );
    assert!(!read("ben-claude", "shared_context").contains("Wombat"));
    assert!(read("ann-claude", "shared_context").contains("Wombat"));
}

/// A public key in real SSH wire format, which the hub parses.
#[cfg(unix)]
fn ed25519_key(seed: u8, comment: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend((0..32).map(|i| seed.wrapping_mul(64).wrapping_add(i)));

    let mut encoded = String::new();
    for chunk in blob.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
        for shift in [18, 12, 6, 0] {
            encoded.push(ALPHABET[((value >> shift) & 0x3f) as usize] as char);
        }
        for _ in chunk.len()..3 {
            encoded.pop();
            encoded.push('=');
        }
    }
    format!("ssh-ed25519 {encoded} {comment}\n")
}
