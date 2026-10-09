//! The signal that the live snapshot changed (live-projects, design 11.3).
//!
//! It is a system notification without a body. Its name carries the
//! identity of the profile: an opaque random value kept in a file of the
//! profile's home. Whoever needs it first, the daemon or a client, makes
//! the file with a create that fails when the file is there, and otherwise
//! reads it; so a client that starts before the daemon listens under the
//! name the daemon will post under. The port is no identity: two profiles
//! can use the same one.
//!
//! At most one signal is posted in two seconds, with one after a burst.
use anyhow::{Result, bail};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

/// The file of a profile's home that holds its identity: the one the
/// clients make and read (`MnemonicProfile.identity()`).
pub const IDENTITY_FILE: &str = "profile.id";
/// The least time between two signals.
pub const EVERY: Duration = Duration::from_secs(2);

/// The identity a file holds, when it holds one: 32 lowercase
/// hexadecimal digits, as the clients write and accept it.
fn read_identity(path: &Path) -> Option<String> {
    let kept = std::fs::read_to_string(path).ok()?;
    let value = kept.trim();
    let hex = value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (value.len() == 32 && hex).then(|| value.to_string())
}

/// The identity of the profile whose home is `home`: read when its file
/// is there, made when it is not. The file is written whole under a name
/// of its own and linked into place, as the clients do it: a reader never
/// sees it half written, and of two that make it at once one wins and
/// both read the same value.
pub fn identity(home: &Path) -> Result<String> {
    let path = home.join(IDENTITY_FILE);
    if let Some(found) = read_identity(&path) {
        return Ok(found);
    }
    // Not there, or not an identity. A client may make it between this
    // look and the link below: the link tells, and what is there then is
    // read again.
    let made = uuid::Uuid::new_v4().simple().to_string();
    let scratch = home.join(format!(
        "{IDENTITY_FILE}.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| -> Result<()> {
        let mut file = options.open(&scratch)?;
        file.write_all(made.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    let linked = written.and_then(|()| match std::fs::hard_link(&scratch, &path) {
        Ok(()) => Ok(made),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match read_identity(&path) {
            Some(found) => Ok(found),
            None => bail!("the identity of the profile is not one this program made"),
        },
        Err(e) => Err(e.into()),
    });
    let _ = std::fs::remove_file(&scratch);
    linked
}

/// The name the signal of a profile is posted under.
pub fn name_of(identity: &str) -> String {
    format!("mnemonic.live.{identity}")
}

/// What posts a signal.
pub trait Post: Send + Sync {
    fn post(&self, name: &str);
}

/// The notification service of the system. Where there is none, a post
/// does nothing.
pub struct System;

impl Post for System {
    #[cfg(target_os = "macos")]
    fn post(&self, name: &str) {
        unsafe extern "C" {
            fn notify_post(name: *const std::ffi::c_char) -> u32;
        }
        if let Ok(name) = std::ffi::CString::new(name) {
            // SAFETY: `name` is a C string that lives through the call,
            // and the service copies what it needs of it.
            unsafe { notify_post(name.as_ptr()) };
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn post(&self, _name: &str) {}
}

/// At most one signal in `EVERY`, and one after a burst: a change that
/// comes too soon after a signal is owed one, posted when the time is up.
#[derive(Debug, Default)]
pub struct Throttle {
    last: Option<Instant>,
    owed: bool,
}

impl Throttle {
    /// The snapshot changed at `now`. Whether a signal is posted now;
    /// when not, one is owed.
    pub fn changed(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) < EVERY => {
                self.owed = true;
                false
            }
            _ => {
                self.last = Some(now);
                self.owed = false;
                true
            }
        }
    }

    /// When the owed signal is due.
    pub fn due(&self) -> Option<Instant> {
        self.last.filter(|_| self.owed).map(|last| last + EVERY)
    }

    /// Time reached `now`. Whether the owed signal is posted now.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.due() {
            Some(due) if now >= due => {
                self.last = Some(now);
                self.owed = false;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "live_signal_tests.rs"]
mod tests;
