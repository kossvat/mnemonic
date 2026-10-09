//! Who drives a Claude Code session: the owner, or a program.
//!
//! The Agent SDKs and the headless CLI stamp `entrypoint` as `sdk-*` on the
//! records of the sessions they run. Not every record carries the field, and
//! a record without it in such a transcript is still the program's: it takes
//! the driver of its transcript, the first entrypoint the transcript names.
//! Records of a session a program ran make no stretch of the owner's work
//! and are not taken as memories, live or from history (live-projects,
//! design 5.1). A transcript that names no entrypoint yet is taken as
//! before. The rule for Codex, whose `session_meta` names the driver, is
//! `CodexOrigin`.
use anyhow::Result;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::ingress::RecordScope;
use super::scope::{ClaudeScope, ProjectScope};

/// What a Claude Code record says of who drives its session: `Some(true)`
/// for a program (`entrypoint` of `sdk-*`), `Some(false)` for any other
/// entrypoint, `None` for a record that names none.
pub fn claude_session_is_automated(line: &str) -> Option<bool> {
    // Cheap reject before parsing: once the driver is known most records
    // are judged by this alone.
    if !line.contains("\"entrypoint\"") {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Entry<'a> {
        #[serde(borrow)]
        entrypoint: Option<Cow<'a, str>>,
    }
    let entry: Entry = serde_json::from_str(line).ok()?;
    Some(entry.entrypoint?.starts_with("sdk-"))
}

/// The most of a transcript read to learn its driver when a poll begins past
/// its start. The first records of a session name its entrypoint; a
/// transcript whose first megabytes name none is taken as before.
const DRIVER_SEARCH_CAP: u64 = 4 << 20;

/// The driver named by the first record before `offset` that names one.
fn first_driver(file: &mut (impl Read + Seek), offset: u64) -> std::io::Result<Option<bool>> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file.take(offset.min(DRIVER_SEARCH_CAP)));
    let mut record = Vec::new();
    loop {
        record.clear();
        if reader.read_until(b'\n', &mut record)? == 0 {
            return Ok(None);
        }
        let driver = std::str::from_utf8(&record)
            .ok()
            .and_then(|line| claude_session_is_automated(line.trim_end_matches('\n')));
        if driver.is_some() {
            return Ok(driver);
        }
    }
}

/// Refuses the records of a Claude Code session a program ran. A record that
/// names an entrypoint is judged by it; one that names none takes the driver
/// of its transcript. The driver is carried per file and generation, and a
/// file first seen past its start learns it from its first records, read from
/// the descriptor the records come from.
#[derive(Debug, Default)]
pub struct ClaudeOrigin {
    committed: HashMap<PathBuf, (u64, Option<bool>)>,
    driver: Option<bool>,
}

impl ClaudeOrigin {
    /// Start a transcript from its first byte, as history reads each one.
    pub fn reset(&mut self) {
        self.driver = None;
    }
}

impl RecordScope for ClaudeOrigin {
    fn begin(
        &mut self,
        file: &mut std::fs::File,
        path: &Path,
        generation: u64,
        offset: u64,
    ) -> Result<()> {
        self.driver = match self.committed.get(path) {
            // What the committed records named, none included: a later
            // record that names one sets it.
            Some((g, driver)) if *g == generation => *driver,
            _ if offset == 0 => None,
            _ => first_driver(file, offset)?,
        };
        Ok(())
    }
    fn admit(&mut self, line: &str) -> bool {
        match claude_session_is_automated(line) {
            Some(automated) => {
                self.driver.get_or_insert(automated);
                !automated
            }
            None => self.driver != Some(true),
        }
    }
    fn commit(&mut self, path: &Path, generation: u64, _next_offset: u64) {
        self.committed
            .insert(path.to_path_buf(), (generation, self.driver));
    }
}

/// What the Claude Code watcher admits: a record of a session the owner
/// drives, and, when the scope is restricted, inside the project roots. Both
/// see every record, so each keeps its state.
pub struct ClaudeSession {
    origin: ClaudeOrigin,
    folder: Option<ClaudeScope>,
}

impl ClaudeSession {
    pub fn new(scope: ProjectScope) -> Self {
        Self {
            origin: ClaudeOrigin::default(),
            folder: scope.is_restricted().then(|| ClaudeScope::new(scope)),
        }
    }
}

impl RecordScope for ClaudeSession {
    fn begin(
        &mut self,
        file: &mut std::fs::File,
        path: &Path,
        generation: u64,
        offset: u64,
    ) -> Result<()> {
        self.origin.begin(file, path, generation, offset)?;
        if let Some(folder) = self.folder.as_mut() {
            folder.begin(file, path, generation, offset)?;
        }
        Ok(())
    }
    fn admit(&mut self, line: &str) -> bool {
        let from_owner = self.origin.admit(line);
        let in_scope = self.folder.as_mut().is_none_or(|folder| folder.admit(line));
        from_owner && in_scope
    }
    fn commit(&mut self, path: &Path, generation: u64, next_offset: u64) {
        self.origin.commit(path, generation, next_offset);
        if let Some(folder) = self.folder.as_mut() {
            folder.commit(path, generation, next_offset);
        }
    }
}
