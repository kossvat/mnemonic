//! Rendering and checking the SSH side of hub access.
//!
//! An agent reaches the hub through one `authorized_keys` line that forces a
//! single command: `serve` with a fixed database and a fixed policy. One line
//! without that restriction is a shell as the hub user, and the owner verbs
//! have no authentication of their own. So the lines are never typed by hand:
//! `grant` renders them, and `lint` fails on anything it did not render.
//! Neither command edits sshd's files; an administrator installs the output.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::mcp::Policy;
use super::types::validate_slug;

/// Key types OpenSSH accepts in `authorized_keys`, with the name each one
/// carries inside its own blob. `rsa-sha2-*` are aliases that authenticate
/// the very same RSA key, so they must resolve to one identity.
const KEY_TYPES: &[(&str, &str)] = &[
    ("ssh-ed25519", "ssh-ed25519"),
    ("ssh-rsa", "ssh-rsa"),
    ("rsa-sha2-256", "ssh-rsa"),
    ("rsa-sha2-512", "ssh-rsa"),
    ("ecdsa-sha2-nistp256", "ecdsa-sha2-nistp256"),
    ("ecdsa-sha2-nistp384", "ecdsa-sha2-nistp384"),
    ("ecdsa-sha2-nistp521", "ecdsa-sha2-nistp521"),
    ("sk-ssh-ed25519@openssh.com", "sk-ssh-ed25519@openssh.com"),
    (
        "sk-ecdsa-sha2-nistp256@openssh.com",
        "sk-ecdsa-sha2-nistp256@openssh.com",
    ),
];

/// The name inside the blob for a declared key type, if it is one we accept.
fn blob_name(key_type: &str) -> Option<&'static str> {
    KEY_TYPES
        .iter()
        .find(|(declared, _)| *declared == key_type)
        .map(|(_, name)| *name)
}

/// The shape of a usable key of this type: how many fields follow the name,
/// and what each one has to look like. A blob with the right length prefixes
/// but the wrong contents is not a key anyone can log in with, and granting
/// access to it only produces a setup that silently never works.
fn check_key_shape(name: &str, fields: &[&[u8]]) -> Result<()> {
    let curve_point = |fields: &[&[u8]], curve: &str, point_len: usize| -> Result<()> {
        ensure!(
            fields[0] == curve.as_bytes(),
            "the key names curve {}, not {curve}",
            String::from_utf8_lossy(fields[0])
        );
        ensure!(
            fields[1].len() == point_len && fields[1].first() == Some(&0x04),
            "the {curve} point must be {point_len} uncompressed bytes"
        );
        Ok(())
    };
    match name {
        "ssh-ed25519" => {
            ensure!(fields.len() == 1, "an ed25519 key has one field");
            ensure!(fields[0].len() == 32, "an ed25519 key is 32 bytes");
        }
        "sk-ssh-ed25519@openssh.com" => {
            ensure!(
                fields.len() == 2,
                "a security-key ed25519 key has two fields"
            );
            ensure!(fields[0].len() == 32, "an ed25519 key is 32 bytes");
            ensure!(!fields[1].is_empty(), "the application string is empty");
        }
        "ssh-rsa" => {
            ensure!(
                fields.len() == 2,
                "an RSA key has an exponent and a modulus"
            );
            let (exponent, modulus) = (canonical_mpint(fields[0]), canonical_mpint(fields[1]));
            ensure!(!exponent.is_empty(), "the RSA exponent is empty");
            // Significant bits, not bytes: a sign-padding byte and leading
            // zero bits are not key strength. OpenSSH refuses under 1024.
            let bits = mpint_bits(&modulus);
            ensure!(
                bits >= 1024,
                "the RSA modulus is {bits} bits, under the 1024 OpenSSH accepts"
            );
        }
        "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" => {
            ensure!(fields.len() == 2, "an ECDSA key has a curve and a point");
            let (curve, point_len) = match name {
                "ecdsa-sha2-nistp256" => ("nistp256", 65),
                "ecdsa-sha2-nistp384" => ("nistp384", 97),
                _ => ("nistp521", 133),
            };
            curve_point(fields, curve, point_len)?;
        }
        "sk-ecdsa-sha2-nistp256@openssh.com" => {
            ensure!(
                fields.len() == 3,
                "a security-key ECDSA key has a curve, a point and an application"
            );
            curve_point(fields, "nistp256", 65)?;
            ensure!(!fields[2].is_empty(), "the application string is empty");
        }
        other => bail!("unsupported key type {other}"),
    }
    Ok(())
}

pub struct GrantRequest<'a> {
    pub db: &'a Path,
    pub bin: &'a Path,
    pub policy_dir: &'a Path,
    pub project: &'a str,
    pub principal: &'a str,
    pub agent: &'a str,
    pub write: bool,
    pub expires_at: Option<&'a str>,
    pub max_session_secs: Option<u64>,
    pub public_key: &'a str,
}

#[derive(Debug, Serialize)]
pub struct Grant {
    pub agent_id: String,
    pub policy_path: PathBuf,
    pub policy_toml: String,
    pub authorized_keys_line: String,
}

/// A path that can sit inside a double-quoted forced command unambiguously.
fn plain_absolute(path: &Path, what: &str) -> Result<String> {
    let text = path
        .to_str()
        .with_context(|| format!("{what} must be UTF-8"))?;
    ensure!(path.is_absolute(), "{what} must be an absolute path");
    ensure!(
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)),
        "{what} may only contain ASCII letters, digits and / . _ -"
    );
    Ok(text.to_owned())
}

/// Exactly one bare public key: `<type> <base64> [comment]`. Pasted options,
/// a second key or a private key are refused.
fn parse_public_key(text: &str) -> Result<(String, String)> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    ensure!(
        lines.len() == 1,
        "the public key file must hold exactly one key, found {} lines",
        lines.len()
    );
    let mut fields = lines[0].split_whitespace();
    let key_type = fields.next().unwrap_or_default();
    ensure!(
        blob_name(key_type).is_some(),
        "the public key file must start with a key type such as ssh-ed25519, not options or a private key"
    );
    let blob = fields.next().context("the public key has no key data")?;
    ensure!(
        blob.len() >= 32
            && blob
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
        "the public key data is not base64"
    );
    Ok((key_type.to_owned(), blob.to_owned()))
}

fn forced_command(bin: &str, db: &str, policy: &str) -> String {
    format!("{bin} shared --db {db} serve --policy {policy}")
}

/// Strict standard base64: the whole token must decode. Stopping at the first
/// `=` would silently accept a key with anything appended, which OpenSSH then
/// refuses, leaving a grant that never works.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|b| **b == b'=').count();
    if padding > 2 || bytes[..bytes.len() - padding].contains(&b'=') {
        return None;
    }
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for byte in &bytes[..bytes.len() - padding] {
        let value = ALPHABET.iter().position(|c| c == byte)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // The bits the padding stands for must be zero, as a real encoder leaves them.
    (acc & ((1 << bits) - 1) == 0).then_some(out)
}

/// One length-prefixed field of the SSH wire format.
fn take_field<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (length, rest) = bytes.split_at_checked(4)?;
    let length = u32::from_be_bytes(length.try_into().ok()?) as usize;
    let (field, rest) = rest.split_at_checked(length)?;
    *bytes = rest;
    Some(field)
}

/// An mpint without its redundant leading zero bytes. OpenSSH accepts several
/// encodings of one RSA key and they all authenticate the same private key,
/// so the base64 as written is not an identity.
fn canonical_mpint(field: &[u8]) -> Vec<u8> {
    let trimmed = field
        .iter()
        .position(|b| *b != 0)
        .map_or(&[][..], |i| &field[i..]);
    let mut out = Vec::with_capacity(trimmed.len() + 1);
    if trimmed.first().is_some_and(|b| b & 0x80 != 0) {
        out.push(0);
    }
    out.extend_from_slice(trimmed);
    out
}

/// Significant bits of a canonical mpint, ignoring any sign-padding byte.
fn mpint_bits(canonical: &[u8]) -> usize {
    match canonical.iter().position(|b| *b != 0) {
        Some(index) => {
            (canonical.len() - index - 1) * 8 + (8 - canonical[index].leading_zeros() as usize)
        }
        None => 0,
    }
}

/// A stable identity for one public key: the same key written another valid
/// way gives the same bytes back.
fn key_identity(key_type: &str, blob: &str) -> Result<Vec<u8>> {
    let expected =
        blob_name(key_type).with_context(|| format!("unsupported key type {key_type}"))?;
    let decoded = base64_decode(blob).context("the public key data is not base64")?;
    let mut rest = decoded.as_slice();
    let name = take_field(&mut rest).context("the public key data is truncated")?;
    ensure!(
        name == expected.as_bytes(),
        "the public key data is a {} key, but the line says {key_type}",
        String::from_utf8_lossy(name)
    );
    let mut fields = Vec::new();
    while let Some(field) = take_field(&mut rest) {
        fields.push(field);
    }
    ensure!(rest.is_empty(), "the public key data has trailing bytes");
    check_key_shape(expected, &fields)?;

    // Length-prefixed, never delimited: a separator byte can occur inside a
    // field, and then two different keys would share one identity.
    let mut identity = Vec::new();
    let mut push = |field: &[u8]| {
        identity.extend_from_slice(&(field.len() as u32).to_be_bytes());
        identity.extend_from_slice(field);
    };
    // The blob's own name, so `ssh-rsa` and `rsa-sha2-256` are one key.
    push(expected.as_bytes());
    for field in fields {
        // RSA carries two mpints; the other types carry fixed-length data.
        if expected == "ssh-rsa" {
            push(&canonical_mpint(field));
        } else {
            push(field);
        }
    }
    Ok(identity)
}

/// Render the policy file and the one `authorized_keys` line for an agent.
pub fn grant(request: &GrantRequest<'_>) -> Result<Grant> {
    validate_slug(request.project, "project")?;
    validate_slug(request.principal, "principal")?;
    validate_slug(request.agent, "agent")?;
    ensure!(
        !request.agent.contains('-'),
        "--agent must not contain a hyphen, or `<principal>-<agent>` would be ambiguous"
    );
    let agent_id = format!("{}-{}", request.principal, request.agent);
    validate_slug(&agent_id, "agent_id")?;
    let db = plain_absolute(request.db, "--db")?;
    let bin = plain_absolute(request.bin, "--bin")?;
    let policy_path = request.policy_dir.join(format!("{agent_id}.toml"));
    let policy = plain_absolute(&policy_path, "--policy-dir")?;
    let (key_type, blob) = parse_public_key(request.public_key)?;
    key_identity(&key_type, &blob)?;

    let mut policy_toml = format!(
        "version = 2\nproject_id = \"{}\"\nprincipal_id = \"{}\"\nagent_id = \"{agent_id}\"\nallow_observations = {}\n",
        request.project, request.principal, request.write
    );
    if let Some(expires_at) = request.expires_at {
        policy_toml.push_str(&format!("expires_at = \"{expires_at}\"\n"));
    }
    if let Some(secs) = request.max_session_secs {
        policy_toml.push_str(&format!("max_session_secs = {secs}\n"));
    }
    // Refuse to hand out a policy the server would reject.
    toml::from_str::<Policy>(&policy_toml)?.validate()?;

    Ok(Grant {
        authorized_keys_line: format!(
            "restrict,command=\"{}\" {key_type} {blob} mnemonic:{}:{agent_id}",
            forced_command(&bin, &db, &policy),
            request.project
        ),
        agent_id,
        policy_path,
        policy_toml,
    })
}

/// Split an `authorized_keys` line on unquoted whitespace.
fn fields(line: &str) -> Vec<String> {
    let (mut out, mut current, mut quoted, mut escaped) = (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        match c {
            _ if escaped => {
                current.push(c);
                escaped = false;
            }
            '\\' if quoted => {
                current.push(c);
                escaped = true;
            }
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Split an options field on unquoted commas.
fn options(field: &str) -> Vec<String> {
    let (mut out, mut current, mut quoted) = (Vec::new(), String::new(), false);
    for c in field.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => out.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    out.push(current);
    out
}

pub struct LintRequest<'a> {
    pub db: &'a Path,
    pub bin: &'a Path,
    pub policy_dir: &'a Path,
    pub authorized_keys: &'a str,
    /// Public keys allowed an unrestricted line: the curator's own login.
    pub owner_keys: &'a [String],
    pub pinned_project: Option<&'a str>,
}

/// Every problem found; an empty list means the file is safe to install.
pub fn lint(request: &LintRequest<'_>) -> Result<Vec<String>> {
    let db = plain_absolute(request.db, "--db")?;
    let bin = plain_absolute(request.bin, "--bin")?;
    // `--policy-dir /p/` and `/p` must behave alike: grant joins, lint compares.
    let policy_dir = plain_absolute(request.policy_dir, "--policy-dir")?
        .trim_end_matches('/')
        .to_owned();
    let owner_keys: HashSet<Vec<u8>> = request
        .owner_keys
        .iter()
        .map(|key| {
            let (key_type, blob) = parse_public_key(key)?;
            key_identity(&key_type, &blob)
        })
        .collect::<Result<_>>()?;
    let (mut problems, mut seen, mut agents) = (Vec::new(), HashSet::new(), HashSet::new());

    for (index, raw) in request.authorized_keys.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut problem = |text: String| problems.push(format!("line {}: {text}", index + 1));
        // The key type sits at a fixed place: first, or straight after one
        // options field. Never search further, or an unrecognised type in
        // front would let a key hide behind a name found in the comment.
        let fields = fields(line);
        let Some(type_at) = (0..2).find(|at| {
            fields
                .get(*at)
                .is_some_and(|field| blob_name(field).is_some())
        }) else {
            problem(
                "no recognisable key type at the start of the line or after its options".into(),
            );
            continue;
        };
        let Some(blob) = fields.get(type_at + 1) else {
            problem("key data is missing".into());
            continue;
        };
        let identity = match key_identity(&fields[type_at], blob) {
            Ok(identity) => identity,
            Err(error) => {
                problem(error.to_string());
                continue;
            }
        };
        if !seen.insert(identity.clone()) {
            problem("the same key appears twice; one key is one agent".into());
        }
        if owner_keys.contains(&identity) {
            continue;
        }
        if type_at != 1 {
            problem(
                "an agent key needs exactly one options field with restrict and command=".into(),
            );
            continue;
        }
        let options = options(&fields[0]);
        if !options.iter().any(|option| option == "restrict") {
            problem("missing `restrict`: the key could open a shell, a PTY or a tunnel".into());
        }
        // Anything after `restrict` that gives a capability back.
        for option in &options {
            let name = option.split('=').next().unwrap_or_default();
            if !matches!(name, "restrict" | "command") {
                problem(format!("option `{name}` is not allowed on an agent key"));
            }
        }
        let prefix = forced_command(&bin, &db, "");
        let commands: Vec<&String> = options
            .iter()
            .filter(|o| o.starts_with("command="))
            .collect();
        let policy_path = match commands.as_slice() {
            [command] => command
                .strip_prefix("command=\"")
                .and_then(|rest| rest.strip_suffix('"'))
                .and_then(|inner| inner.strip_prefix(&prefix))
                .map(Path::new)
                .filter(|path| {
                    path.parent() == Some(Path::new(&policy_dir))
                        && path
                            .extension()
                            .is_some_and(|extension| extension == "toml")
                })
                .map(Path::to_path_buf),
            _ => None,
        };
        let Some(policy_path) = policy_path else {
            problem(format!(
                "the forced command must be exactly `{prefix}{policy_dir}/<agent_id>.toml`"
            ));
            continue;
        };
        match Policy::load(&policy_path) {
            Err(error) => problem(format!("policy {}: {error}", policy_path.display())),
            Ok(policy) => {
                if policy.version != 2 {
                    problem("policy is not version 2, so it names no person to revoke".into());
                }
                if Some(policy.agent_id.as_str())
                    != policy_path.file_stem().and_then(|stem| stem.to_str())
                {
                    problem("policy file name does not match its agent_id".into());
                }
                if !agents.insert(policy.agent_id.clone()) {
                    problem(format!("agent_id {} is granted twice", policy.agent_id));
                }
                if request
                    .pinned_project
                    .is_some_and(|pinned| pinned != policy.project_id)
                {
                    problem(format!(
                        "policy is for project {}, the database is pinned elsewhere",
                        policy.project_id
                    ));
                }
            }
        }
    }
    if problems.is_empty() && agents.is_empty() {
        bail!("no agent keys found: refusing to call an empty file safe");
    }
    Ok(problems)
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
