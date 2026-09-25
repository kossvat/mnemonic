//! Real process: profile selection through `MNEMONIC_HOME` and `--home`.
//! Every child gets a fake `$HOME`, so the developer's own store is never read.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        // `/tmp`, not `temp_dir()`: the profile socket path must stay under
        // the Unix limit.
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = PathBuf::from("/tmp").join(format!("mn-cli-{tag}-{}", &id[..8]));
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const OWNER_SENTINEL: &str = "dedup_threshold = 0.11";

/// A fake user home holding a decoy owner config that must never be consulted.
fn fake_home(root: &Path) -> PathBuf {
    let home = root.join("user");
    let config_dir = home.join(".config/mnemonic");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        format!("[classifier]\nimportance_threshold = 0.4\n{OWNER_SENTINEL}\n"),
    )
    .unwrap();
    home
}

fn mnemonic(user_home: &Path, env_home: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
    cmd.env("HOME", user_home).env_remove("MNEMONIC_HOME");
    if let Some(value) = env_home {
        cmd.env("MNEMONIC_HOME", value);
    }
    cmd.args(args).output().unwrap()
}

fn owner_config(user_home: &Path) -> String {
    std::fs::read_to_string(user_home.join(".config/mnemonic/config.toml")).unwrap()
}

#[test]
fn init_under_the_variable_writes_only_inside_the_profile() {
    let root = Temp::new("env");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let before = owner_config(&user_home);

    let out = mnemonic(&user_home, profile.to_str(), &["init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let written = std::fs::read_to_string(profile.join("config.toml")).unwrap();
    assert!(written.contains(&format!(
        "db_path = \"{}\"",
        profile.join("memory.db").display()
    )));
    assert!(!written.contains(OWNER_SENTINEL));
    assert!(
        !written.contains(user_home.to_str().unwrap()),
        "owner path leaked:\n{written}"
    );
    assert_eq!(owner_config(&user_home), before);
    assert!(!user_home.join(".mnemonic").exists());
}

#[test]
fn flag_selects_the_profile_when_the_launcher_drops_the_environment() {
    let root = Temp::new("flag");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");

    let out = mnemonic(
        &user_home,
        None,
        &["--home", profile.to_str().unwrap(), "init"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(profile.join("config.toml").exists());
    assert!(!user_home.join(".mnemonic").exists());

    // The flag is global: it also works after the subcommand.
    let after = root.0.join("client2");
    let out = mnemonic(
        &user_home,
        None,
        &["init", "--home", after.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(after.join("config.toml").exists());
}

#[test]
fn unusable_selections_fail_closed_and_touch_nothing() {
    let root = Temp::new("closed");
    let user_home = fake_home(&root.0);
    let before = owner_config(&user_home);
    let default_store = user_home.join(".mnemonic");
    let nested = default_store.join("client");
    let other = root.0.join("other");
    let third = root.0.join("third");

    let cases: [(Option<&str>, Vec<&str>); 5] = [
        (Some(""), vec!["init"]),
        (Some("relative/dir"), vec!["init"]),
        (Some(default_store.to_str().unwrap()), vec!["init"]),
        (Some(nested.to_str().unwrap()), vec!["init"]),
        (
            Some(other.to_str().unwrap()),
            vec!["--home", third.to_str().unwrap(), "init"],
        ),
    ];
    for (env_home, args) in cases {
        let out = mnemonic(&user_home, env_home, &args);
        assert!(
            !out.status.success(),
            "{env_home:?} {args:?} must be refused"
        );
    }
    assert_eq!(owner_config(&user_home), before);
    assert!(
        !default_store.exists(),
        "a refused profile must not create the default store"
    );
    assert!(!other.join("config.toml").exists());
    assert!(!third.join("config.toml").exists());
}

#[test]
fn context_refuses_to_write_through_a_symlink_out_of_the_profile() {
    let root = Temp::new("ctx");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let work = root.0.join("work");
    let outside = root.0.join("outside.md");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(&outside, "owner notes").unwrap();

    let home_arg = profile.to_str().unwrap();
    assert!(
        mnemonic(&user_home, None, &["--home", home_arg, "init"])
            .status
            .success()
    );

    // The default destination is <memory-files>/-<urlencoded cwd>/CONTEXT.md.
    let encoded = work.to_str().unwrap().replace('/', "%2F");
    let dest_dir = profile.join("memory-files").join(format!("-{encoded}"));
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::os::unix::fs::symlink(&outside, dest_dir.join("CONTEXT.md")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_mnemonic"))
        .env("HOME", &user_home)
        .env_remove("MNEMONIC_HOME")
        .current_dir(&work)
        .args(["--home", home_arg, "context"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "context must refuse the symlinked destination"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "owner notes");
}

#[test]
fn init_with_a_project_root_scopes_capture_and_prints_agent_wiring() {
    let root = Temp::new("setup");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let project = root.0.join("code/demoapp");
    std::fs::create_dir_all(&project).unwrap();
    let home_arg = profile.to_str().unwrap();

    // Refused roots write nothing: a missing folder, and one that swallows $HOME.
    for bad in [root.0.join("code/missing"), user_home.clone()] {
        let out = mnemonic(
            &user_home,
            None,
            &[
                "--home",
                home_arg,
                "init",
                "--project-root",
                bad.to_str().unwrap(),
            ],
        );
        assert!(!out.status.success(), "{} must be refused", bad.display());
        assert!(!profile.join("config.toml").exists());
    }

    let out = mnemonic(
        &user_home,
        None,
        &[
            "--home",
            home_arg,
            "init",
            "--project-root",
            project.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(profile.join("config.toml")).unwrap();
    assert!(written.contains("conversation_enabled = true"));
    assert!(written.contains("codex_enabled = true"));
    assert!(written.contains(&format!("project_roots = [\"{}\"]", project.display())));
    assert!(written.contains("git_enabled = false"));

    // The printed wiring pins the store in argv with absolute paths.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pinned = format!("--home {} mcp", profile.display());
    assert!(stdout.contains(&pinned), "{stdout}");
    assert!(stdout.contains(env!("CARGO_BIN_EXE_mnemonic")), "{stdout}");

    // A second run must not wipe the profile unless forced.
    let again = mnemonic(&user_home, None, &["--home", home_arg, "init"]);
    assert!(!again.status.success());
    assert_eq!(
        std::fs::read_to_string(profile.join("config.toml")).unwrap(),
        written
    );
    let forced = mnemonic(&user_home, None, &["--home", home_arg, "init", "--force"]);
    assert!(forced.status.success());
}

#[test]
fn a_symlinked_project_root_keeps_both_spellings() {
    let root = Temp::new("alias");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let real = root.0.join("volumes/demoapp");
    let link = root.0.join("demoapp");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let out = mnemonic(
        &user_home,
        None,
        &[
            "--home",
            profile.to_str().unwrap(),
            "init",
            "--project-root",
            link.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(profile.join("config.toml")).unwrap();
    // Sessions started through either path record that path as their cwd.
    assert!(written.contains(real.to_str().unwrap()), "{written}");
    assert!(
        written.contains(&format!("\"{}\"", link.display())),
        "{written}"
    );
}

#[test]
fn a_relative_project_root_keeps_its_absolute_spelling() {
    let root = Temp::new("relalias");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let real = root.0.join("volumes/demoapp");
    let work = root.0.join("work");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::os::unix::fs::symlink(&real, work.join("demoapp")).unwrap();
    // The shell stands in `desk`, a link to `work`: the OS reports `work`,
    // the shell and every agent started there say `desk`.
    let desk = root.0.join("desk");
    std::os::unix::fs::symlink(&work, &desk).unwrap();

    // The way a human types it: `./demoapp` from the folder holding the link.
    let out = Command::new(env!("CARGO_BIN_EXE_mnemonic"))
        .env("HOME", &user_home)
        .env_remove("MNEMONIC_HOME")
        .env("PWD", &desk)
        .current_dir(&desk)
        .args(["--home", profile.to_str().unwrap()])
        .args(["init", "--project-root", "./demoapp"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(profile.join("config.toml")).unwrap();
    assert!(written.contains(real.to_str().unwrap()), "{written}");
    // The shell's spelling of the link, absolute and without the `./`.
    assert!(
        written.contains(&format!("\"{}\"", desk.join("demoapp").display())),
        "{written}"
    );
    assert!(!written.contains("./demoapp"), "{written}");
}

#[test]
fn a_file_is_not_a_project_root() {
    let root = Temp::new("rootfile");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let file = root.0.join("Cargo.toml");
    std::fs::write(&file, "[package]\n").unwrap();
    let out = mnemonic(
        &user_home,
        None,
        &[
            "--home",
            profile.to_str().unwrap(),
            "init",
            "--project-root",
            file.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());
    assert!(!profile.join("config.toml").exists());
}

#[test]
fn history_is_queued_only_for_a_running_daemon() {
    let root = Temp::new("histd");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let project = root.0.join("code");
    std::fs::create_dir_all(&project).unwrap();
    let home = profile.to_str().unwrap();
    assert!(
        mnemonic(
            &user_home,
            None,
            &[
                "--home",
                home,
                "init",
                "--project-root",
                project.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    // Counting works without a daemon; queueing does not.
    assert!(
        mnemonic(&user_home, None, &["--home", home, "ingest-history"])
            .status
            .success()
    );
    let out = mnemonic(
        &user_home,
        None,
        &["--home", home, "ingest-history", "--apply"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not running"));
}

#[test]
fn an_export_cannot_follow_a_symlink_out_of_the_profile() {
    let root = Temp::new("sink");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("client");
    let outside = root.0.join("outside-vault");
    std::fs::create_dir_all(&outside).unwrap();
    let home = profile.to_str().unwrap();
    assert!(
        mnemonic(&user_home, None, &["--home", home, "init"])
            .status
            .success()
    );
    let save = |text: &str| {
        mnemonic(
            &user_home,
            None,
            &["--home", home, "save", "--title", "T", text],
        )
    };
    let saved = save("a note written before the export is turned on");
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );

    // Turn the Obsidian export on, then plant a symlink under its folder.
    let config = profile.join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("obsidian_enabled = false", "obsidian_enabled = true"),
    )
    .unwrap();
    let vault = profile.join("obsidian");
    std::fs::create_dir_all(&vault).unwrap();
    std::os::unix::fs::symlink(&outside, vault.join("Agents")).unwrap();

    // Neither the bulk export nor the live sink may follow it.
    let export = mnemonic(&user_home, None, &["--home", home, "backfill-obsidian"]);
    assert!(!export.status.success());
    let _ = save("a note written with the export on");
    let leaked = walk(&outside);
    assert!(leaked.is_empty(), "written outside the profile: {leaked:?}");
}

/// Everything under `dir`, folders included: an empty folder created
/// through the symlink is a write outside the profile too.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

#[test]
fn the_printed_setup_survives_a_path_with_spaces_and_quotes() {
    let root = Temp::new("quote");
    let user_home = fake_home(&root.0);
    let profile = root.0.join("my client's store");
    let home = profile.to_str().unwrap();
    let out = mnemonic(&user_home, None, &["--home", home, "init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The shell line runs as printed (asking for `doctor`, not a daemon).
    let start = stdout
        .lines()
        .find(|line| line.trim_end().ends_with("start -d"))
        .unwrap();
    let doctor = start.trim().replace("start -d", "doctor");
    let ran = Command::new("sh")
        .arg("-c")
        .arg(&doctor)
        .env("HOME", &user_home)
        .env_remove("MNEMONIC_HOME")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("Isolated profile"),
        "{doctor}\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );

    // The JSON snippet parses and carries the exact path.
    let json = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("\"mnemonic\":"))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&format!("{{{json}}}")).unwrap();
    assert_eq!(value["mnemonic"]["args"][1], home);

    // So does the TOML one.
    let toml_lines: Vec<&str> = stdout
        .lines()
        .skip_while(|line| !line.contains("[mcp_servers.mnemonic]"))
        .take(3)
        .map(str::trim)
        .collect();
    let parsed: toml::Value = toml_lines.join("\n").parse().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["mnemonic"]["args"][1].as_str(),
        Some(home)
    );
}

#[test]
fn a_symlinked_selector_and_its_target_are_one_profile() {
    let root = Temp::new("selector");
    let user_home = fake_home(&root.0);
    let real = root.0.join("store");
    std::fs::create_dir_all(&real).unwrap();
    let link = root.0.join("store-link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = mnemonic(
        &user_home,
        link.to_str(),
        &["--home", real.to_str().unwrap(), "init"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Two different directories still disagree.
    let other = root.0.join("other");
    let out = mnemonic(
        &user_home,
        other.to_str(),
        &["--home", real.to_str().unwrap(), "doctor"],
    );
    assert!(!out.status.success());
}
