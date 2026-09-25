use super::*;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

/// Real SSH wire format, because the code parses it: a length-prefixed type
/// name followed by the 32-byte key.
fn ed25519_key(seed: u8, comment: &str) -> String {
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend((0..32).map(|i| seed.wrapping_add(i)));
    format!("ssh-ed25519 {} {comment}", base64_encode(&blob))
}

/// An RSA key line from its two components.
fn rsa_key_from(exponent: &[u8], modulus: &[u8]) -> String {
    let mut blob = Vec::new();
    blob.extend_from_slice(&7u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-rsa");
    for field in [exponent, modulus] {
        blob.extend_from_slice(&(field.len() as u32).to_be_bytes());
        blob.extend_from_slice(field);
    }
    format!("ssh-rsa {} rsa@laptop", base64_encode(&blob))
}

/// A genuinely 1024-bit modulus: the top bit of the first byte is set, so
/// all 128 bytes count. OpenSSH accepts nothing smaller.
fn modulus() -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..128).map(|i: u8| i.wrapping_add(1)).collect();
    bytes[0] = 0xc7;
    bytes
}

/// The same RSA key with a redundant leading zero on its modulus: another
/// base64 string that OpenSSH accepts as the very same key.
fn rsa_key(padded: bool) -> String {
    let mut bytes = vec![0x00u8; usize::from(padded)];
    bytes.extend(modulus());
    rsa_key_from(&[0x01, 0x00, 0x01], &bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
        for shift in [18, 12, 6, 0] {
            out.push(ALPHABET[((value >> shift) & 0x3f) as usize] as char);
        }
        // 3 bytes make 4 characters, 2 make 3 plus one pad, 1 makes 2 plus two.
        let keep = chunk.len() + 1;
        out.truncate(out.len() - (4 - keep));
        out.extend(std::iter::repeat_n('=', 4 - keep));
    }
    out
}

struct Fixture {
    root: PathBuf,
    db: PathBuf,
    bin: PathBuf,
    policies: PathBuf,
    key: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("mnemonic-access-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let policies = root.join("policies");
        fs::DirBuilder::new().mode(0o700).create(&policies).unwrap();
        Self {
            db: root.join("shared.db"),
            bin: root.join("mnemonic"),
            policies,
            root,
            key: ed25519_key(1, "ann@laptop"),
        }
    }
    fn db(&self) -> PathBuf {
        self.db.clone()
    }
    fn bin(&self) -> PathBuf {
        self.bin.clone()
    }
    fn policies(&self) -> PathBuf {
        self.policies.clone()
    }
    fn request<'a>(&'a self, principal: &'a str, agent: &'a str, write: bool) -> GrantRequest<'a> {
        GrantRequest {
            db: &self.db,
            bin: &self.bin,
            policy_dir: &self.policies,
            project: "alpha",
            principal,
            agent,
            write,
            expires_at: None,
            max_session_secs: None,
            public_key: &self.key,
        }
    }
    /// Install a grant the way an administrator would.
    fn install(&self, grant: &Grant) {
        fs::write(&grant.policy_path, &grant.policy_toml).unwrap();
        fs::set_permissions(&grant.policy_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn lint(&self, authorized_keys: &str, owner_keys: &[String]) -> Result<Vec<String>> {
        lint(&LintRequest {
            db: &self.db(),
            bin: &self.bin(),
            policy_dir: &self.policies(),
            authorized_keys,
            owner_keys,
            pinned_project: Some("alpha"),
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_rendered_line_always_forces_one_serve_command() {
    let f = Fixture::new();
    let rendered = grant(&f.request("ann", "claude", false)).unwrap();
    assert_eq!(rendered.agent_id, "ann-claude");
    assert!(
        rendered
            .authorized_keys_line
            .starts_with("restrict,command=\"")
    );
    assert!(rendered.authorized_keys_line.contains(&format!(
        "{} shared --db {} serve --policy {}",
        f.bin().display(),
        f.db().display(),
        rendered.policy_path.display()
    )));
    // The key is carried over untouched and the comment names the agent.
    let blob = f.key.split(' ').nth(1).unwrap();
    assert!(rendered.authorized_keys_line.contains(blob));
    assert!(
        rendered
            .authorized_keys_line
            .ends_with("mnemonic:alpha:ann-claude")
    );
    assert!(!rendered.authorized_keys_line.contains('\n'));

    // The policy names the person and defaults to read-only.
    assert!(rendered.policy_toml.contains("version = 2"));
    assert!(rendered.policy_toml.contains("principal_id = \"ann\""));
    assert!(rendered.policy_toml.contains("allow_observations = false"));
    let write = grant(&f.request("ann", "contrib", true)).unwrap();
    assert!(write.policy_toml.contains("allow_observations = true"));
}

#[test]
fn a_pasted_key_file_that_is_not_one_bare_key_is_refused() {
    let f = Fixture::new();
    let one = ed25519_key(1, "ann");
    let two = ed25519_key(2, "ben");
    // Valid base64 that is not an ed25519 key body.
    let mislabelled = format!("ssh-ed25519 {} mislabelled", base64_encode(b"not a key"));
    for bad in [
        "",
        "   \n\n",
        &format!("{one}\n{two}"),
        &format!("command=\"/bin/sh\" {one}"),
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "ssh-ed25519",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA$(whoami)IsNotBase64Data00 ann",
        "ssh-ed25519 AAAA short",
        &mislabelled,
    ] {
        let mut request = f.request("ann", "claude", false);
        request.public_key = bad;
        assert!(grant(&request).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn a_hostile_path_or_name_cannot_break_out_of_the_forced_command() {
    let f = Fixture::new();
    for db in [
        "relative/shared.db",
        "/hub/sha red.db",
        "/hub/\"; sh #.db",
        "/hub/$(whoami).db",
    ] {
        let db = PathBuf::from(db);
        let mut request = f.request("ann", "claude", false);
        request.db = &db;
        assert!(grant(&request).is_err(), "{} must be refused", db.display());
    }
    for name in ["Ann", "ann claude", "ann/../root", ""] {
        let mut request = f.request("ann", "claude", false);
        request.agent = name;
        assert!(grant(&request).is_err(), "{name:?} must be refused");
    }
}

#[test]
fn an_agent_name_with_a_hyphen_is_refused() {
    let f = Fixture::new();
    // `ann` + `ops-claude` and `ann-ops` + `claude` would both read as
    // `ann-ops-claude`, so two people could share one writer id and quota.
    let mut request = f.request("ann", "ops-claude", false);
    assert!(grant(&request).is_err());
    request.agent = "claude";
    assert!(grant(&request).is_ok());
}

#[test]
fn lint_accepts_only_what_grant_rendered() {
    let f = Fixture::new();
    let other = ed25519_key(2, "ben@laptop");
    let ann = grant(&f.request("ann", "claude", false)).unwrap();
    f.install(&ann);
    let ben = {
        let mut request = f.request("ben", "claude", false);
        request.public_key = &other;
        grant(&request).unwrap()
    };
    f.install(&ben);

    let clean = format!(
        "# hub keys\n{}\n{}\n",
        ann.authorized_keys_line, ben.authorized_keys_line
    );
    assert_eq!(f.lint(&clean, &[]).unwrap(), Vec::<String>::new());

    // The curator's own unrestricted login is allowed only when declared.
    let with_owner = format!("{}\n{other}\n", ann.authorized_keys_line);
    assert!(!f.lint(&with_owner, &[]).unwrap().is_empty());
    assert_eq!(f.lint(&with_owner, &[other]).unwrap(), Vec::<String>::new());
}

#[test]
fn lint_fails_on_every_way_a_line_gives_more_than_serve() {
    let f = Fixture::new();
    let other = ed25519_key(2, "ben@laptop");
    let ann = grant(&f.request("ann", "claude", false)).unwrap();
    f.install(&ann);
    let line = ann.authorized_keys_line.clone();
    let policy = ann.policy_path.display().to_string();

    let cases: Vec<(&str, String)> = vec![
        ("no options at all", f.key.clone()),
        ("no restrict", line.replacen("restrict,", "", 1)),
        ("no forced command", format!("restrict {}", f.key)),
        (
            "a capability handed back",
            line.replacen("restrict,", "restrict,pty,", 1),
        ),
        (
            "agent forwarding",
            line.replacen("restrict,", "restrict,agent-forwarding,", 1),
        ),
        (
            "another binary",
            line.replacen(&f.bin().display().to_string(), "/bin/sh", 1),
        ),
        (
            "another database",
            line.replacen(&f.db().display().to_string(), "/hub/other.db", 1),
        ),
        (
            "a policy outside the directory",
            line.replacen(&policy, "/tmp/evil.toml", 1),
        ),
        (
            "a policy in a subdirectory",
            line.replacen("ann-claude.toml", "sub/ann-claude.toml", 1),
        ),
        (
            "a policy that is not installed",
            line.replacen("ann-claude.toml", "ghost.toml", 1),
        ),
        (
            "a second command option",
            line.replacen("restrict,", "restrict,command=\"/bin/sh\",", 1),
        ),
        ("the same key twice", format!("{line}\n{line}")),
    ];
    for (what, keys) in cases {
        let problems = f.lint(&keys, &[]);
        assert!(
            problems.as_ref().map(|p| !p.is_empty()).unwrap_or(true),
            "{what} must be reported: {problems:?}"
        );
    }

    // Two people must not end up with the same agent_id.
    let twin = {
        let mut request = f.request("ann", "claude", false);
        request.public_key = &other;
        grant(&request).unwrap()
    };
    let duplicated = format!("{line}\n{}", twin.authorized_keys_line);
    assert!(!f.lint(&duplicated, &[]).unwrap().is_empty());
}

#[test]
fn the_same_key_in_another_encoding_is_still_the_same_key() {
    let f = Fixture::new();
    let plain = rsa_key(false);
    let padded = rsa_key(true);
    assert_ne!(plain, padded, "the two lines differ as text");

    let ann = {
        let mut request = f.request("ann", "claude", false);
        request.public_key = &plain;
        grant(&request).unwrap()
    };
    f.install(&ann);
    let ben = {
        let mut request = f.request("ben", "claude", false);
        request.public_key = &padded;
        grant(&request).unwrap()
    };
    f.install(&ben);

    // One private key must never authenticate two separate grants.
    let problems = f
        .lint(
            &format!(
                "{}\n{}\n",
                ann.authorized_keys_line, ben.authorized_keys_line
            ),
            &[],
        )
        .unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("the same key appears twice")),
        "{problems:?}"
    );

    // The curator's own key is recognised through a re-encoding too: their
    // line is written one way and declared the other.
    let g = Fixture::new();
    let agent = grant(&g.request("ann", "claude", false)).unwrap();
    g.install(&agent);
    let owner_line = format!("{}\n{plain}\n", agent.authorized_keys_line);
    assert_eq!(
        g.lint(&owner_line, std::slice::from_ref(&padded)).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn a_trailing_slash_on_the_policy_directory_changes_nothing() {
    let f = Fixture::new();
    let with_slash = PathBuf::from(format!("{}/", f.policies().display()));
    let mut request = f.request("ann", "claude", false);
    request.policy_dir = &with_slash;
    let rendered = grant(&request).unwrap();
    f.install(&rendered);
    assert_eq!(
        rendered.policy_path,
        f.policies().join("ann-claude.toml"),
        "grant must not produce a doubled separator"
    );
    assert_eq!(
        lint(&LintRequest {
            db: &f.db(),
            bin: &f.bin(),
            policy_dir: &with_slash,
            authorized_keys: &rendered.authorized_keys_line,
            owner_keys: &[],
            pinned_project: Some("alpha"),
        })
        .unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn lint_reports_a_policy_that_does_not_belong_to_this_hub() {
    let f = Fixture::new();
    let ann = grant(&f.request("ann", "claude", false)).unwrap();

    // Version 1: no person, so nobody can be revoked.
    fs::write(
        &ann.policy_path,
        "version=1\nproject_id='alpha'\nagent_id='ann-claude'\n",
    )
    .unwrap();
    fs::set_permissions(&ann.policy_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!f.lint(&ann.authorized_keys_line, &[]).unwrap().is_empty());

    // A policy for another project on a pinned database.
    fs::write(
        &ann.policy_path,
        "version=2\nproject_id='beta'\nprincipal_id='ann'\nagent_id='ann-claude'\n",
    )
    .unwrap();
    fs::set_permissions(&ann.policy_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!f.lint(&ann.authorized_keys_line, &[]).unwrap().is_empty());

    // A file name that does not match the agent it configures.
    f.install(&ann);
    let renamed = f.policies().join("ann-codex.toml");
    fs::copy(&ann.policy_path, &renamed).unwrap();
    let line = ann
        .authorized_keys_line
        .replacen("ann-claude.toml", "ann-codex.toml", 1);
    assert!(!f.lint(&line, &[]).unwrap().is_empty());
}

#[test]
fn an_empty_or_owner_only_file_is_never_called_safe() {
    let f = Fixture::new();
    let owner = ed25519_key(9, "owner");
    assert!(f.lint("", &[]).is_err());
    assert!(f.lint("# only a comment\n", &[]).is_err());
    assert!(f.lint(&owner, std::slice::from_ref(&owner)).is_err());
}

#[test]
fn two_different_keys_never_share_one_identity() {
    let f = Fixture::new();
    // The same bytes split differently between the exponent and the modulus.
    // These are two distinct keys and must stay distinguishable, however the
    // identity is built.
    let tail = modulus();
    let mut shifted = vec![0x01, 0x3a];
    shifted.extend(&tail);
    let left = rsa_key_from(&[0x03], &shifted);
    let right = rsa_key_from(&[0x03, 0x3a, 0x01], &tail);
    assert_ne!(left, right);

    let agent = grant(&f.request("ann", "claude", false)).unwrap();
    f.install(&agent);
    // Declaring one as the curator's own login must not admit the other as an
    // unrestricted line.
    let file = format!("{}\n{right}\n", agent.authorized_keys_line);
    assert!(
        !f.lint(&file, std::slice::from_ref(&left))
            .unwrap()
            .is_empty(),
        "an undeclared key must not pass as the declared one"
    );
    assert_eq!(
        f.lint(&file, std::slice::from_ref(&right)).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn a_key_cannot_hide_behind_a_name_in_the_comment() {
    let f = Fixture::new();
    let agent = grant(&f.request("ann", "claude", false)).unwrap();
    f.install(&agent);
    let owner = ed25519_key(9, "owner");
    let attacker = rsa_key(false);
    let attacker_blob = attacker.split(' ').nth(1).unwrap();
    let owner_blob = owner.split(' ').nth(1).unwrap();

    // OpenSSH reads the first two fields as the key; the rest is a comment.
    // Naming the owner's key type in the comment must not exempt the line.
    for line in [
        format!("rsa-sha2-256 {attacker_blob} ssh-ed25519 {owner_blob}"),
        format!("ssh-rsa {attacker_blob} ssh-ed25519 {owner_blob}"),
        // An unknown type up front must not let a later field pass as the key.
        format!("x-unknown-type {attacker_blob} ssh-ed25519 {owner_blob}"),
    ] {
        let file = format!("{}\n{line}\n", agent.authorized_keys_line);
        let problems = f.lint(&file, std::slice::from_ref(&owner)).unwrap();
        assert!(
            !problems.is_empty(),
            "{line} must be reported: {problems:?}"
        );
    }
}

#[test]
fn an_rsa_alias_is_the_same_key_as_plain_ssh_rsa() {
    let f = Fixture::new();
    let plain = rsa_key(false);
    let alias = plain.replacen("ssh-rsa", "rsa-sha2-512", 1);

    // The alias is usable as an agent key on its own.
    let aliased = {
        let mut request = f.request("ann", "claude", false);
        request.public_key = &alias;
        grant(&request).unwrap()
    };
    f.install(&aliased);
    assert!(aliased.authorized_keys_line.contains("rsa-sha2-512"));

    // The same RSA key under two names is still one key, not two grants.
    let twin = {
        let mut request = f.request("ben", "claude", false);
        request.public_key = &plain;
        grant(&request).unwrap()
    };
    f.install(&twin);
    assert!(
        !f.lint(
            &format!(
                "{}\n{}\n",
                aliased.authorized_keys_line, twin.authorized_keys_line
            ),
            &[]
        )
        .unwrap()
        .is_empty(),
        "the same RSA key under two names must be caught"
    );

    // The curator's line may use either spelling of their declared key.
    let g = Fixture::new();
    let agent = grant(&g.request("ann", "claude", false)).unwrap();
    g.install(&agent);
    assert_eq!(
        g.lint(
            &format!("{}\n{alias}\n", agent.authorized_keys_line),
            std::slice::from_ref(&plain)
        )
        .unwrap(),
        Vec::<String>::new(),
        "declaring it once covers either spelling"
    );
}

#[test]
fn a_blob_that_is_not_a_usable_key_is_refused() {
    let f = Fixture::new();
    let field = |bytes: &[u8]| {
        let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(bytes);
        out
    };
    let line = |name: &str, declared: &str, fields: Vec<Vec<u8>>| {
        let mut blob = field(name.as_bytes());
        for part in fields {
            blob.extend(part);
        }
        format!("{declared} {} broken", base64_encode(&blob))
    };

    let short_point = {
        let mut point = vec![0x04];
        point.extend([7u8; 40]);
        point
    };
    for bad in [
        // Well-formed length prefixes, but nothing anyone can log in with.
        line("ssh-ed25519", "ssh-ed25519", vec![field(&[1, 2, 3, 4, 5])]),
        line("ssh-ed25519", "ssh-ed25519", vec![]),
        line(
            "ssh-ed25519",
            "ssh-ed25519",
            vec![field(&[7u8; 32]), field(b"extra")],
        ),
        // A 128-bit RSA modulus: far under what OpenSSH accepts.
        line(
            "ssh-rsa",
            "ssh-rsa",
            vec![field(&[1, 0, 1]), field(&[9u8; 16])],
        ),
        line("ssh-rsa", "rsa-sha2-256", vec![field(&[1, 0, 1])]),
        // The curve name does not match the declared type.
        line(
            "ecdsa-sha2-nistp256",
            "ecdsa-sha2-nistp256",
            vec![field(b"nistp384"), field(&short_point)],
        ),
    ] {
        let mut request = f.request("ann", "claude", false);
        request.public_key = &bad;
        assert!(grant(&request).is_err(), "{bad} must be refused");

        // The lint must not approve such a line either.
        let agent = grant(&f.request("ann", "codex", false)).unwrap();
        f.install(&agent);
        let file = format!("{}\n{bad}\n", agent.authorized_keys_line);
        // Either the lint refuses the file outright, or it reports the line.
        // Both are safe; silently approving it is not.
        assert!(
            f.lint(&file, std::slice::from_ref(&bad))
                .is_ok_and(|problems| !problems.is_empty())
                || f.lint(&file, &[])
                    .is_ok_and(|problems| !problems.is_empty()),
            "{bad} must be reported"
        );
    }
}

#[test]
fn key_data_that_openssh_would_reject_is_refused() {
    let f = Fixture::new();
    let valid = ed25519_key(1, "ann");
    let (kind, blob) = {
        let mut fields = valid.split(' ');
        (fields.next().unwrap(), fields.next().unwrap())
    };

    for (what, line) in [
        // Anything appended after the padding: OpenSSH refuses the token.
        ("trailing junk", format!("{kind} {blob}=junk ann")),
        ("trailing character", format!("{kind} {blob}x ann")),
        (
            "truncated token",
            format!("{kind} {} ann", &blob[..blob.len() - 1]),
        ),
        (
            "padding in the middle",
            format!("{kind} {}=A== ann", &blob[..blob.len() - 4]),
        ),
        (
            "not base64",
            format!("{kind} {} ann", "!".repeat(blob.len())),
        ),
    ] {
        let mut request = f.request("ann", "claude", false);
        request.public_key = &line;
        assert!(grant(&request).is_err(), "{what} must be refused");
    }

    // An RSA key one bit short of 1024 is not a 1024-bit key.
    let mut short = modulus();
    short[0] = 0x01;
    let short = rsa_key_from(&[0x01, 0x00, 0x01], &short);
    let mut request = f.request("ann", "claude", false);
    request.public_key = &short;
    assert!(
        grant(&request).is_err(),
        "a 1017-bit modulus must be refused"
    );

    // The full-strength key still works.
    let full = rsa_key(false);
    request.public_key = &full;
    assert!(grant(&request).is_ok());
}
