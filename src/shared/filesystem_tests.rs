use super::*;
use crate::shared::store::SharedStore;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mnemonic-paths-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn dir(&self, name: &str, mode: u32) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }
    fn policy(&self, dir: &Path) -> PathBuf {
        let path = dir.join("policy.toml");
        fs::write(&path, "version=1\nproject_id='alpha'\nagent_id='scout'\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn private_temp_and_trusted_directory_aliases_resolve_without_following_leaf() {
    let f = Fixture::new();
    let hub = f.dir("hub", 0o700);
    let absolute = f.0.join("absolute");
    symlink(&hub, &absolute).unwrap();
    let relative = f.0.join("relative");
    symlink("absolute", &relative).unwrap();
    let expected = hub.canonicalize().unwrap().join("shared.db");
    assert_eq!(
        trusted_file_path(&relative.join("shared.db")).unwrap(),
        expected
    );
    assert!(SharedStore::open(&relative.join("shared.db")).is_ok());
    let policy = f.policy(&hub);
    assert!(open_policy(&absolute.join("policy.toml")).is_ok());
    assert!(open_policy(&policy).is_ok());
    assert_eq!(
        trusted_file_path(&hub.join("../relative/shared.db")).unwrap(),
        expected
    );
    // The final DB/policy name must never be canonicalized into a trusted file.
    symlink(&expected, hub.join("leaf.db")).unwrap();
    assert!(SharedStore::open(&hub.join("leaf.db")).is_err());
    symlink(&policy, hub.join("leaf.toml")).unwrap();
    assert!(open_policy(&hub.join("leaf.toml")).is_err());
}

#[test]
fn writable_immediate_parents_are_rejected_without_database_creation_or_chmod() {
    let f = Fixture::new();
    for mode in [0o770, 0o777, 0o1770, 0o1777] {
        let dir = f.dir(&format!("mode-{mode:o}"), mode);
        let policy = f.policy(&dir);
        let db = dir.join("shared.db");
        assert!(SharedStore::open(&db).is_err(), "{mode:o}");
        assert!(open_policy(&policy).is_err(), "{mode:o}");
        assert!(!db.exists());
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o7777, mode);
    }
}

#[test]
fn trusted_child_does_not_hide_a_writable_nonsticky_ancestor() {
    let f = Fixture::new();
    for mode in [0o770, 0o777] {
        let ancestor = f.dir(&format!("mode-{mode:o}"), mode);
        let hub = ancestor.join("hub");
        fs::DirBuilder::new().mode(0o700).create(&hub).unwrap();
        let policy = f.policy(&hub);
        let db = hub.join("shared.db");
        assert!(SharedStore::open(&db).is_err());
        assert!(open_policy(&policy).is_err());
        assert!(!db.exists());
    }
}

#[test]
fn sticky_ancestor_with_trusted_private_child_supports_temporary_stores() {
    let f = Fixture::new();
    let sticky = f.dir("sticky", 0o1777);
    let hub = sticky.join("hub");
    fs::DirBuilder::new().mode(0o700).create(&hub).unwrap();
    let policy = f.policy(&hub);
    assert!(SharedStore::open(&hub.join("shared.db")).is_ok());
    assert!(open_policy(&policy).is_ok());
    // Sticky protection also applies to a trusted owner's symlink entry.
    symlink(&hub, sticky.join("alias")).unwrap();
    assert!(open_policy(&sticky.join("alias/policy.toml")).is_ok());
}

#[test]
fn canonical_safe_target_does_not_bypass_unsafe_alias_or_dotdot_route() {
    let f = Fixture::new();
    let hub = f.dir("hub", 0o700);
    let unsafe_dir = f.dir("writable", 0o777);
    symlink(&hub, unsafe_dir.join("alias")).unwrap();
    symlink(unsafe_dir.join("alias"), f.0.join("indirect")).unwrap();
    for route in [
        unsafe_dir.join("alias/shared.db"),
        f.0.join("indirect/shared.db"),
        unsafe_dir.join("../hub/shared.db"),
    ] {
        assert_eq!(
            route.parent().unwrap().canonicalize().unwrap(),
            hub.canonicalize().unwrap()
        );
        assert!(trusted_file_path(&route).is_err(), "{}", route.display());
        assert!(SharedStore::open(&route).is_err());
    }
    assert!(!hub.join("shared.db").exists());
}

#[test]
fn directory_ownership_requires_root_or_service_even_with_safe_or_sticky_modes() {
    let f = Fixture::new();
    for mode in [0o700, 0o755, 0o1777] {
        let path = f.dir(&format!("mode-{mode:o}"), mode);
        let meta = fs::metadata(&path).unwrap();
        assert!(check_directory(&meta, meta.uid(), true).is_ok());
        // Use an alternate service identity to exercise real filesystem metadata
        // without requiring chown/root or modifying global process credentials.
        if meta.uid() != 0 {
            assert!(check_directory(&meta, meta.uid() + 1, true).is_err());
        }
    }
    assert!(check_owner(1001, 1002).is_err());
    assert!(check_owner(0, 1002).is_ok());
    assert!(check_directory(&fs::metadata("/").unwrap(), 1002, true).is_ok());
}

#[test]
fn policy_descriptor_retains_permission_checks_and_rejects_nonregular_files() {
    let f = Fixture::new();
    let policy = f.policy(&f.0);
    fs::set_permissions(&policy, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(open_policy(&policy).is_err());
    fs::set_permissions(&policy, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(open_policy(&policy).is_ok());
    assert!(open_policy(&f.dir("not-a-policy", 0o700)).is_err());
    let fifo = f.0.join("policy-fifo");
    use std::os::unix::ffi::OsStrExt;
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    assert!(open_policy(&fifo).is_err());
}

#[test]
fn missing_directory_and_symlink_cycles_fail_without_creating_paths() {
    let f = Fixture::new();
    assert!(SharedStore::open(&f.0.join("missing/shared.db")).is_err());
    assert!(!f.0.join("missing").exists());
    symlink("cycle-b", f.0.join("cycle-a")).unwrap();
    symlink("cycle-a", f.0.join("cycle-b")).unwrap();
    let error = trusted_file_path(&f.0.join("cycle-a/shared.db")).unwrap_err();
    assert!(error.to_string().contains("too many"));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_system_var_alias_is_supported_for_private_temporary_directories() {
    // Do not assume the caller's TMPDIR uses /var (it may be /tmp or custom).
    let directory = Path::new("/var/tmp").join(format!("mnemonic-var-{}", uuid::Uuid::new_v4()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let f = Fixture(directory);
    let canonical = f.0.canonicalize().unwrap();
    let tail = canonical.strip_prefix("/private/var").unwrap();
    let alias = Path::new("/var").join(tail).join("shared.db");
    assert_eq!(
        trusted_file_path(&alias).unwrap(),
        canonical.join("shared.db")
    );
    assert!(SharedStore::open(&alias).is_ok());
}
