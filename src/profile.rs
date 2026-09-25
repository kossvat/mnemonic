//! Isolated profiles: one hard-separated store per `MNEMONIC_HOME` / `--home`.
//!
//! The default install is a single-owner store under `~/.mnemonic`. Tags and
//! project links inside a store organise memories, but they are not a
//! boundary: every MCP tool, the dedup check and the graph read the whole
//! database. A profile is the boundary. With `--home /abs/dir` (or
//! `MNEMONIC_HOME=/abs/dir`) the process keeps its config, databases, socket,
//! pid, log and auth token inside that directory and never falls back to the
//! owner's default paths.
//!
//! Everything here fails closed. A selection that is present but unusable is
//! an error, never a silent fallback to the default store: a launcher that
//! expands an undefined variable must not hand an agent the owner's memory.
//!
//! This separates stores, not operating-system users. Any process running as
//! the same uid can still open another profile's files directly; give an
//! untrusted agent its own uid (see `mnemonic shared`) when that matters.

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Selects the isolated profile directory for this process.
pub const HOME_ENV: &str = "MNEMONIC_HOME";

/// macOS caps `sun_path` at 104 bytes and Linux at 108. Bind fails late and
/// obscurely past that, so reject it while loading the config instead.
const MAX_SOCKET_PATH_BYTES: usize = 100;

static SELECTED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Pin the profile for this process from the `--home` flag.
///
/// Launchers drop `env` blocks far more often than they drop arguments, and a
/// dropped variable silently selects the owner's default store. A client's MCP
/// entry should therefore pass `--home`; the variable stays as a fallback and
/// must agree with the flag when both are present.
pub fn select(flag: Option<PathBuf>) -> Result<()> {
    let user_home = dirs::home_dir();
    let from_env = parse(std::env::var_os(HOME_ENV), user_home.as_deref())?;
    let from_flag = match flag {
        Some(path) => parse(Some(path.into_os_string()), user_home.as_deref())?,
        None => None,
    };
    let chosen = match (from_flag, from_env) {
        (Some(flag), Some(env)) if !same_location(&flag, &env) => bail!(
            "--home {} and {HOME_ENV} {} disagree",
            flag.display(),
            env.display()
        ),
        (Some(flag), _) => Some(flag),
        (None, env) => env,
    };
    let _ = SELECTED.set(chosen);
    Ok(())
}

/// Do two spellings name the same directory? `/tmp/p` and `/private/tmp/p`
/// on macOS do, and so do a symlink and its target. A directory that does not
/// exist yet can only be compared as written.
fn same_location(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// The isolated profile selected for this process, if any.
pub fn active() -> Result<Option<PathBuf>> {
    match SELECTED.get() {
        Some(selected) => Ok(selected.clone()),
        None => parse(std::env::var_os(HOME_ENV), dirs::home_dir().as_deref()),
    }
}

fn parse(raw: Option<OsString>, user_home: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        bail!("{HOME_ENV} is set but empty; unset it or point it at a dedicated directory");
    }
    let home = PathBuf::from(raw);
    if !home.is_absolute() {
        bail!(
            "{HOME_ENV} must be an absolute path, got {}",
            home.display()
        );
    }
    if home.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("{HOME_ENV} must not contain `..`: {}", home.display());
    }
    ensure_dedicated(&home, &home, user_home)?;
    Ok(Some(home))
}

/// Directories that belong to the owner's default install. A profile may not
/// be, contain, or live inside any of them: `MNEMONIC_HOME=~/.mnemonic` would
/// otherwise resolve to exactly the owner's database, socket and token.
fn reserved_dirs(user_home: &Path) -> [PathBuf; 2] {
    [
        user_home.join(".mnemonic"),
        user_home.join(".config/mnemonic"),
    ]
}

fn ensure_dedicated(home: &Path, shown: &Path, user_home: Option<&Path>) -> Result<()> {
    let Some(user_home) = user_home else {
        return Ok(());
    };
    // Never the user's home directory or one of its ancestors: the directory
    // is locked to 0700 and filled with databases.
    if user_home.starts_with(home) {
        bail!(
            "{HOME_ENV} must be a dedicated directory, not {}",
            shown.display()
        );
    }
    for reserved in reserved_dirs(user_home) {
        let reserved = reserved.canonicalize().unwrap_or(reserved);
        if home.starts_with(&reserved) || reserved.starts_with(home) {
            bail!(
                "{HOME_ENV} {} overlaps the default store {}; use a separate directory such as ~/.mnemonic-profiles/<name>",
                shown.display(),
                reserved.display()
            );
        }
    }
    Ok(())
}

/// Create the profile directory owner-only and return its canonical path.
pub fn prepare(home: &Path) -> Result<PathBuf> {
    prepare_for(home, dirs::home_dir().as_deref())
}

fn prepare_for(home: &Path, user_home: Option<&Path>) -> Result<PathBuf> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("cannot create {HOME_ENV} {}", home.display()))?;
    let resolved = home
        .canonicalize()
        .with_context(|| format!("cannot resolve {HOME_ENV} {}", home.display()))?;
    // `parse` checked the spelling; a symlink or a case variant can still
    // resolve to the user's home or the default store. Check the real target
    // before locking it to 0700.
    let user_home = user_home.map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
    ensure_dedicated(&resolved, home, user_home.as_deref())?;
    tighten_dir(&resolved)?;
    Ok(resolved)
}

#[cfg(unix)]
fn tighten_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot lock {} to owner-only", path.display()))
}

#[cfg(not(unix))]
fn tighten_dir(_path: &Path) -> Result<()> {
    Ok(())
}

/// Reject a state path that would land outside the canonical profile `home`.
///
/// The nearest existing ancestor is canonicalised, so a symlink planted inside
/// the profile cannot redirect a database or token to another store; a
/// dangling symlink is refused because creating the file would follow it.
pub fn ensure_inside(home: &Path, label: &str, path: &Path) -> Result<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!(
            "{label} must be an absolute path without `..` in an isolated profile: {}",
            path.display()
        );
    }
    let mut ancestor = path;
    let resolved = loop {
        match ancestor.canonicalize() {
            Ok(resolved) => break resolved,
            Err(_) if ancestor.symlink_metadata().is_ok() => {
                bail!("{label}: {} is a dangling symlink", ancestor.display())
            }
            Err(_) => match ancestor.parent() {
                Some(parent) => ancestor = parent,
                None => bail!("{label} cannot be resolved: {}", path.display()),
            },
        }
    };
    if !resolved.starts_with(home) {
        bail!(
            "{label} = {} is outside the isolated profile {}",
            path.display(),
            home.display()
        );
    }
    Ok(())
}

pub fn ensure_socket_fits(path: &Path) -> Result<()> {
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH_BYTES {
        bail!(
            "socket path is {len} bytes, over the {MAX_SOCKET_PATH_BYTES} byte Unix limit; \
             use a shorter {HOME_ENV}: {}",
            path.display()
        );
    }
    Ok(())
}

/// Overlay the user's `config.toml` on the profile defaults, table by table,
/// so a key the user left out keeps its in-profile default instead of the
/// serde default that points at the owner's `~/.mnemonic`.
pub fn merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(slot) => merge(slot, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn unset_selects_the_default_store() {
        assert_eq!(parse(None, None).unwrap(), None);
    }

    #[test]
    fn set_but_unusable_values_fail_closed() {
        let user_home = Path::new("/Users/someone");
        for raw in [
            "",
            "relative/dir",
            "/abs/../escape",
            "/Users/someone",
            "/Users",
            "/",
        ] {
            assert!(
                parse(Some(raw.into()), Some(user_home)).is_err(),
                "{raw:?} must be rejected, not fall back to the default store"
            );
        }
    }

    #[test]
    fn the_default_store_is_never_a_profile() {
        let user_home = Path::new("/Users/someone");
        for raw in [
            "/Users/someone/.mnemonic",
            "/Users/someone/.mnemonic/",
            "/Users/someone/.mnemonic/profiles/client",
            "/Users/someone/.config/mnemonic",
            "/Users/someone/.config",
        ] {
            assert!(
                parse(Some(raw.into()), Some(user_home)).is_err(),
                "{raw:?} overlaps the owner's default store"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn two_spellings_of_one_directory_agree() {
        let root = temp_dir("alias");
        let real = root.join("profile");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(same_location(&link, &real));
        assert!(same_location(&real, &real));
        assert!(!same_location(&real, &root.join("other")));
        // Neither exists yet: only an identical spelling agrees.
        assert!(same_location(&root.join("new"), &root.join("new")));
        assert!(!same_location(&root.join("new"), &root.join("newer")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dedicated_absolute_directory_is_accepted() {
        let home = parse(
            Some("/Users/someone/.mnemonic-profiles/client".into()),
            Some(Path::new("/Users/someone")),
        )
        .unwrap();
        assert_eq!(
            home,
            Some(PathBuf::from("/Users/someone/.mnemonic-profiles/client"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn prepare_creates_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("prepare");
        let home = prepare_for(&root.join("profile"), None).unwrap();
        let mode = std::fs::metadata(&home).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_the_user_home_is_not_a_dedicated_directory() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("homelink");
        let user_home = root.join("user");
        std::fs::create_dir_all(&user_home).unwrap();
        std::fs::set_permissions(&user_home, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.join("profile");
        std::os::unix::fs::symlink(&user_home, &link).unwrap();

        assert!(prepare_for(&link, Some(&user_home)).is_err());
        let mode = std::fs::metadata(&user_home).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "the refused target must not be re-permissioned"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_the_default_store_is_not_a_profile() {
        let root = temp_dir("storelink");
        let user_home = root.join("user");
        let store = user_home.join(".mnemonic");
        std::fs::create_dir_all(&store).unwrap();
        let link = root.join("client");
        std::os::unix::fs::symlink(&store, &link).unwrap();
        assert!(prepare_for(&link, Some(&user_home)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_outside_the_profile_are_rejected() {
        let root = temp_dir("inside");
        let home = prepare_for(&root.join("profile"), None).unwrap();
        ensure_inside(&home, "db_path", &home.join("memory.db")).unwrap();
        ensure_inside(&home, "db_path", &home.join("new/sub/memory.db")).unwrap();
        assert!(ensure_inside(&home, "db_path", &root.join("other/memory.db")).is_err());
        assert!(ensure_inside(&home, "db_path", Path::new("memory.db")).is_err());
        assert!(ensure_inside(&home, "db_path", &home.join("../other/memory.db")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_the_profile_cannot_escape_it() {
        let root = temp_dir("symlink");
        let home = prepare_for(&root.join("profile"), None).unwrap();
        let other = root.join("other-store");
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&other, home.join("link")).unwrap();
        assert!(ensure_inside(&home, "db_path", &home.join("link/memory.db")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_state_file_is_rejected() {
        let root = temp_dir("dangling");
        let home = prepare_for(&root.join("profile"), None).unwrap();
        // Target does not exist yet: a later create would land outside.
        std::os::unix::fs::symlink(root.join("owner/auth.token"), home.join("auth.token")).unwrap();
        assert!(ensure_inside(&home, "ui.token_file", &home.join("auth.token")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn overlong_socket_path_is_rejected() {
        ensure_socket_fits(Path::new("/tmp/p/mnemonic.sock")).unwrap();
        let long = PathBuf::from(format!("/{}/mnemonic.sock", "d".repeat(120)));
        assert!(ensure_socket_fits(&long).is_err());
    }

    #[test]
    fn merge_keeps_defaults_for_keys_the_user_left_out() {
        let mut base: toml::Value =
            toml::from_str("[ui]\nenabled = false\ntoken_file = \"/p/auth.token\"\n").unwrap();
        let overlay: toml::Value = toml::from_str("[ui]\nenabled = true\n").unwrap();
        merge(&mut base, overlay);
        assert_eq!(base["ui"]["enabled"].as_bool(), Some(true));
        assert_eq!(base["ui"]["token_file"].as_str(), Some("/p/auth.token"));
    }
}
