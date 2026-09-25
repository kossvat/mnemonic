//! Project scope for the transcript watchers.
//!
//! Claude Code and Codex keep every project's transcripts under one global
//! folder, so pointing a watcher at that folder captures every project. A
//! scope narrows capture to sessions whose working directory is inside one of
//! the configured project roots. The working directory is read from the
//! transcript itself: Claude Code stamps `cwd` on every user and assistant
//! record; Codex records it in the leading `session_meta` line and again in a
//! `turn_context` record whenever a resumed session moves to another folder.
//!
//! A restricted scope fails closed: a record or session without a readable
//! working directory is dropped.

use anyhow::Result;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use super::ingress::RecordScope;

#[derive(Debug, Clone, Default)]
pub struct ProjectScope {
    roots: Vec<PathBuf>,
}

impl ProjectScope {
    /// Scope limited to `roots`. An empty list means unrestricted, which is
    /// the default install's behaviour.
    pub fn new(roots: &[PathBuf]) -> Self {
        let mut resolved: Vec<PathBuf> = Vec::new();
        for root in roots {
            // Transcripts record the path the agent saw, which may be either
            // spelling of a symlinked checkout.
            for candidate in [Some(root.clone()), root.canonicalize().ok()]
                .into_iter()
                .flatten()
            {
                if !resolved.contains(&candidate) {
                    resolved.push(candidate);
                }
            }
        }
        Self { roots: resolved }
    }

    pub fn is_restricted(&self) -> bool {
        !self.roots.is_empty()
    }

    /// Is a session running in `cwd` inside this scope?
    pub fn allows(&self, cwd: Option<&Path>) -> bool {
        if !self.is_restricted() {
            return true;
        }
        let Some(cwd) = cwd else {
            return false;
        };
        if !cwd.is_absolute() || cwd.components().any(|c| matches!(c, Component::ParentDir)) {
            return false;
        }
        // Component-wise: `/code/demo` does not admit `/code/demoapp`.
        self.roots.iter().any(|root| cwd.starts_with(root))
    }
}

/// Working directory stamped on a Claude Code transcript record.
pub fn claude_record_cwd(line: &str) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(PathBuf::from(value.get("cwd")?.as_str()?))
}

/// How one transcript format tells which folder its records belong to, read
/// record by record in file order.
pub trait FolderState: Clone + Default {
    /// Advance past one complete record.
    fn observe(&mut self, line: &str);
    /// The folder the next record belongs to, if known.
    fn current(&self) -> Option<&Path>;
    /// Nothing later in the file can change the answer: replay may stop.
    fn settled(&self) -> bool {
        false
    }
}

/// A Claude Code session belongs to the folder it was LAUNCHED in, the first
/// folder its transcript names. Later records carry the agent's current
/// folder, which follows its `cd`: a session opened in a monorepo that steps
/// into the project folder would otherwise start counting as the project.
/// A resume or /compact writes a new transcript stamped with the folder the
/// new process was launched in, so the rule holds for those too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeLaunch(Option<PathBuf>);

impl FolderState for ClaudeLaunch {
    fn observe(&mut self, line: &str) {
        // Cheap reject: most records are skipped before any parsing once the
        // launch folder is known, and records without the key never parse.
        if self.0.is_none() && line.contains("\"cwd\"") {
            self.0 = claude_record_cwd(line);
        }
    }
    fn current(&self) -> Option<&Path> {
        self.0.as_deref()
    }
    fn settled(&self) -> bool {
        self.0.is_some()
    }
}

/// The folder a Codex rollout is working in, record by record. The leading
/// `session_meta` names it and each `turn_context` renames it when a resumed
/// session moves. Between turns it is unknown until the next `turn_context`:
/// a session resumed elsewhere writes its injected user-role instructions
/// before announcing where it now is. A turn that never finished (a crash or
/// a killed terminal) is caught by the next `task_started`: the folder of an
/// unfinished turn does not carry over into a resume.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexFolder {
    cwd: Option<PathBuf>,
    turn_open: bool,
}

enum CodexMark {
    SessionMeta(PathBuf),
    TurnContext(PathBuf),
    TaskStarted,
    TurnEnded,
}

impl FolderState for CodexFolder {
    fn observe(&mut self, line: &str) {
        match codex_mark(line) {
            Some(CodexMark::SessionMeta(cwd)) => {
                self.cwd = Some(cwd);
                self.turn_open = false;
            }
            Some(CodexMark::TurnContext(cwd)) => self.cwd = Some(cwd),
            Some(CodexMark::TaskStarted) => {
                if self.turn_open {
                    self.cwd = None;
                }
                self.turn_open = true;
            }
            Some(CodexMark::TurnEnded) => {
                self.cwd = None;
                self.turn_open = false;
            }
            None => {}
        }
    }
    fn current(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }
}

fn codex_mark(line: &str) -> Option<CodexMark> {
    // Cheap reject before parsing: almost every line is a message.
    if ![
        "session_meta",
        "turn_context",
        "task_started",
        "task_complete",
        "turn_aborted",
    ]
    .iter()
    .any(|marker| line.contains(marker))
    {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let cwd = || {
        value
            .get("payload")?
            .get("cwd")?
            .as_str()
            .map(PathBuf::from)
    };
    match value.get("type")?.as_str()? {
        "session_meta" => cwd().map(CodexMark::SessionMeta),
        "turn_context" => cwd().map(CodexMark::TurnContext),
        "event_msg" => match value.get("payload")?.get("type")?.as_str()? {
            "task_started" => Some(CodexMark::TaskStarted),
            "task_complete" | "turn_aborted" => Some(CodexMark::TurnEnded),
            _ => None,
        },
        _ => None,
    }
}

/// The state in force at byte `offset`, replaying the complete records
/// before it. Used when a daemon resumes a file it read in an earlier run.
pub fn replay<S: FolderState>(
    transcript: &mut (impl Read + Seek),
    offset: u64,
) -> std::io::Result<S> {
    let mut state = S::default();
    transcript.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(transcript.take(offset));
    let mut record = Vec::new();
    while !state.settled() {
        record.clear();
        if reader.read_until(b'\n', &mut record)? == 0 {
            break;
        }
        // Same rule as the live reader: malformed UTF-8 is skipped.
        if let Ok(line) = std::str::from_utf8(&record) {
            state.observe(line.trim_end_matches('\n'));
        }
    }
    Ok(state)
}

/// Admits a record only when the folder its transcript is in lies inside the
/// project roots. The folder is carried from one poll to the next, per file.
pub struct FileScope<S> {
    scope: ProjectScope,
    /// State at the end of the last committed poll: (generation, offset).
    committed: HashMap<PathBuf, (u64, u64, S)>,
    current: S,
}

pub type ClaudeScope = FileScope<ClaudeLaunch>;
pub type CodexScope = FileScope<CodexFolder>;

impl<S: FolderState> FileScope<S> {
    pub fn new(scope: ProjectScope) -> Self {
        Self {
            scope,
            committed: HashMap::new(),
            current: S::default(),
        }
    }

    /// Start a transcript from its first byte, as history reads each one.
    pub fn reset(&mut self) {
        self.current = S::default();
    }
}

impl<S: FolderState> RecordScope for FileScope<S> {
    fn begin(
        &mut self,
        file: &mut std::fs::File,
        path: &Path,
        generation: u64,
        offset: u64,
    ) -> Result<()> {
        // Reload from the last COMMITTED state: a poll that failed after
        // advancing `current` must not leak its half-read state forward.
        self.current = match self.committed.get(path) {
            Some((g, o, state)) if *g == generation && *o == offset => state.clone(),
            // A new or rewritten transcript: nothing is known before byte 0.
            _ if offset == 0 => S::default(),
            // First poll since this daemon started, or the file moved on in
            // a way this process did not see: replay what came before.
            _ => replay(file, offset)?,
        };
        Ok(())
    }
    fn admit(&mut self, line: &str) -> bool {
        self.current.observe(line);
        self.scope.allows(self.current.current())
    }
    fn commit(&mut self, path: &Path, generation: u64, next_offset: u64) {
        self.committed.insert(
            path.to_path_buf(),
            (generation, next_offset, self.current.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(roots: &[&str]) -> ProjectScope {
        ProjectScope::new(&roots.iter().map(PathBuf::from).collect::<Vec<_>>())
    }

    #[test]
    fn unrestricted_scope_allows_everything() {
        let scope = ProjectScope::default();
        assert!(!scope.is_restricted());
        assert!(scope.allows(None));
        assert!(scope.allows(Some(Path::new("/anywhere"))));
    }

    #[test]
    fn restricted_scope_admits_only_sessions_inside_a_root() {
        let scope = scope(&["/code/demoapp", "/code/site"]);
        assert!(scope.allows(Some(Path::new("/code/demoapp"))));
        assert!(scope.allows(Some(Path::new("/code/demoapp/.claude/worktrees/x"))));
        assert!(scope.allows(Some(Path::new("/code/site/app"))));

        assert!(!scope.allows(Some(Path::new("/code/demoapp-old"))));
        assert!(!scope.allows(Some(Path::new("/code/demo"))));
        assert!(!scope.allows(Some(Path::new("/code"))));
        assert!(!scope.allows(Some(Path::new("/code/other"))));
    }

    #[test]
    fn restricted_scope_fails_closed_on_missing_or_odd_paths() {
        let scope = scope(&["/code/demoapp"]);
        assert!(!scope.allows(None));
        assert!(!scope.allows(Some(Path::new("code/demoapp"))));
        assert!(!scope.allows(Some(Path::new("/code/demoapp/../other"))));
    }

    #[test]
    fn claude_cwd_is_read_from_the_record() {
        let line = r#"{"type":"user","cwd":"/code/demoapp","message":{"content":"x"}}"#;
        assert_eq!(
            claude_record_cwd(line),
            Some(PathBuf::from("/code/demoapp"))
        );
        assert_eq!(claude_record_cwd(r#"{"type":"user"}"#), None);
        assert_eq!(claude_record_cwd("not json"), None);
    }

    #[test]
    fn the_codex_folder_follows_session_meta_turns_and_turn_ends() {
        let head = r#"{"type":"session_meta","payload":{"cwd":"/code/demoapp"}}"#;
        let moved = r#"{"type":"turn_context","payload":{"cwd":"/code/private"}}"#;
        let done = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        // A user quoting the words in a message is not a record of that type.
        let quoted = r#"{"type":"response_item","payload":{"text":"turn_context task_complete"}}"#;

        let mut folder = CodexFolder::default();
        assert_eq!(folder.current(), None);
        folder.observe(head);
        assert_eq!(folder.current(), Some(Path::new("/code/demoapp")));
        folder.observe(quoted);
        assert_eq!(folder.current(), Some(Path::new("/code/demoapp")));
        folder.observe(done);
        assert_eq!(folder.current(), None, "unknown between turns");
        folder.observe(moved);
        assert_eq!(folder.current(), Some(Path::new("/code/private")));
    }

    #[test]
    fn replaying_a_prefix_gives_the_folder_in_force_at_that_byte() {
        let dir = std::env::temp_dir().join(format!("mn-scope-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let head = "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/code/demoapp\"}}\n";
        let done = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        let moved = "{\"type\":\"turn_context\",\"payload\":{\"cwd\":\"/code/private\"}}\n";
        let path = dir.join("rollout.jsonl");
        std::fs::write(&path, format!("{head}{done}{moved}")).unwrap();

        let at = |offset: usize| {
            let mut file = std::fs::File::open(&path).unwrap();
            replay::<CodexFolder>(&mut file, offset as u64).unwrap()
        };
        assert_eq!(at(0).current(), None);
        assert_eq!(at(head.len()).current(), Some(Path::new("/code/demoapp")));
        assert_eq!(at(head.len() + done.len()).current(), None);
        assert_eq!(
            at(head.len() + done.len() + moved.len()).current(),
            Some(Path::new("/code/private"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod scoped_ingress_tests;
