//! Shared by the tests that start the real binary.
use std::path::PathBuf;

/// Where a test puts what needs a short path (a profile's socket has to fit
/// the Unix limit): `/tmp`, unless a runner names a directory of its own in
/// `MNEMONIC_TEST_SOCKET_DIR`. A runner that names one promised it: a
/// directory that is not there fails the test instead of sending it back
/// to `/tmp`.
pub fn short_root() -> PathBuf {
    let Some(named) = std::env::var_os("MNEMONIC_TEST_SOCKET_DIR") else {
        return PathBuf::from("/tmp");
    };
    let root = PathBuf::from(named);
    assert!(
        root.is_absolute() && root.is_dir(),
        "MNEMONIC_TEST_SOCKET_DIR names no directory"
    );
    root
}
