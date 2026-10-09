//! What the rules read of a transcript record, in both formats.
//!
//! A record is read borrowed, without building a value of the whole line:
//! its time, the folder it names, and for a call of a tool the class of
//! the tool and the projects its input names. The projects are looked for
//! in the raw text of the call's input and never in a result.
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::{Deserializer, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::borrow::Cow;

use super::lookup::{Named, ProjectMap};
use super::resolver::{Facts, Folder, ToolClass};
use crate::plans::location::Field;

#[path = "facts_location.rs"]
mod location;
pub(crate) use location::{claude_with_location, codex_with_location};

/// What a record is, for the state of a live session and for the owner's
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// The owner typed it.
    Prompt,
    /// A call of a tool.
    Call,
    /// The agent closed its turn: the owner has the move.
    TurnEnd,
    #[default]
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Call => "call",
            Self::TurnEnd => "turn_end",
            Self::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "prompt" => Self::Prompt,
            "call" => Self::Call,
            "turn_end" => Self::TurnEnd,
            _ => Self::Other,
        }
    }
}

/// A record as the rules and the clock see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Observed {
    /// When the record was written, when it says so.
    pub at: Option<DateTime<Utc>>,
    pub facts: Facts,
    pub kind: Kind,
    /// What the record says of who drives its session: a program
    /// (`Some(true)`), the owner, or nothing.
    pub program: Option<bool>,
}

/// The time of a record, to the second. The store keeps seconds, and a
/// walk resumed from what it kept must meet the times an uninterrupted
/// walk meets: a pause is as long for one as for the other.
fn time(stamp: Option<&str>) -> Option<DateTime<Utc>> {
    let at = DateTime::parse_from_rfc3339(stamp?).ok()?;
    DateTime::from_timestamp(at.timestamp(), 0)
}

fn folder(map: &ProjectMap, cwd: Option<&str>) -> Folder {
    match cwd {
        None => Folder::Unsaid,
        Some(cwd) => match map.project_of(cwd) {
            Some(id) => Folder::Project(id),
            None => Folder::Outside,
        },
    }
}

/// The stronger class of two calls in one record: a write over the shell
/// over a read.
fn stronger(a: ToolClass, b: ToolClass) -> ToolClass {
    match (a, b) {
        (ToolClass::Write, _) | (_, ToolClass::Write) => ToolClass::Write,
        (ToolClass::Shell, _) | (_, ToolClass::Shell) => ToolClass::Shell,
        _ => ToolClass::Read,
    }
}

/// Values of `turnOrigin` that a program, not the owner, began the turn with.
pub const MACHINE_TURN_ORIGINS: &[&str] = &["task_notification", "peer", "sdk"];

/// How the app begins the texts it writes into the user's role, for
/// records that carry no field saying who began the turn: a notice of a
/// finished task, the summary a compaction writes, the wrapper of a slash
/// command, a message of another session, the mark of an interrupted
/// request.
const WRITTEN_BY_THE_APP: &[&str] = &[
    "<task-notification>",
    "This session is being continued from a previous conversation",
    "<command-name>",
    "<command-message>",
    "<local-command-",
    "Another Claude session sent a message",
    "<cross-session-message",
    "[Request interrupted by user",
];

pub fn is_written_by_the_app(text: &str) -> bool {
    let text = text.trim_start();
    WRITTEN_BY_THE_APP.iter().any(|head| text.starts_with(head))
}

// ---------------------------------------------------------------- Claude Code

/// The tools of Claude Code that change a file, and its shell. Any other
/// tool reads.
fn claude_class(name: &str) -> ToolClass {
    match name {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => ToolClass::Write,
        "Bash" => ToolClass::Shell,
        _ => ToolClass::Read,
    }
}

#[derive(Deserialize)]
struct ClaudeRecord<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    cwd: Field<'a>,
    #[serde(rename = "gitBranch", borrow, default)]
    branch: Field<'a>,
    #[serde(borrow, default)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    entrypoint: Option<Cow<'a, str>>,
    #[serde(rename = "isSidechain", default)]
    sidechain: Option<bool>,
    #[serde(rename = "isMeta", default)]
    meta: Option<bool>,
    #[serde(rename = "isCompactSummary", default)]
    compact_summary: Option<bool>,
    #[serde(rename = "promptSource", borrow, default)]
    prompt_source: Option<Cow<'a, str>>,
    #[serde(rename = "turnOrigin", borrow, default)]
    turn_origin: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    message: Option<ClaudeMessage<'a>>,
}

#[derive(Deserialize)]
struct ClaudeMessage<'a> {
    #[serde(borrow, default)]
    stop_reason: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    content: Content<'a>,
}

/// What is read of the content of a message: its calls of tools, and the
/// text it opens with.
#[derive(Default)]
struct Content<'a> {
    calls: Vec<Block<'a>>,
    text: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    text: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    input: Option<&'a RawValue>,
}

impl<'de: 'a, 'a> Deserialize<'de> for Content<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Blocks<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de: 'a, 'a> Visitor<'de> for Blocks<'a> {
            type Value = Content<'a>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the content of a message")
            }
            fn visit_borrowed_str<E>(self, text: &'de str) -> Result<Self::Value, E> {
                Ok(Content {
                    text: Some(Cow::Borrowed(text)),
                    ..Content::default()
                })
            }
            fn visit_str<E>(self, text: &str) -> Result<Self::Value, E> {
                Ok(Content {
                    text: Some(Cow::Owned(text.to_owned())),
                    ..Content::default()
                })
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Content::default())
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut content = Content::default();
                while let Some(block) = seq.next_element::<Block<'a>>()? {
                    match block.kind.as_deref() {
                        Some("tool_use") => content.calls.push(block),
                        Some("text" | "input_text" | "output_text") if content.text.is_none() => {
                            content.text = block.text;
                        }
                        _ => {}
                    }
                }
                Ok(content)
            }
        }
        deserializer.deserialize_any(Blocks(std::marker::PhantomData))
    }
}

impl ClaudeRecord<'_> {
    /// Whether the owner typed this record: a text in the user's role, in
    /// a session the owner drives, that no program put there. The same
    /// fields capture reads to tell the owner's words.
    fn is_prompt(&self, text: Option<&str>) -> bool {
        let Some(text) = text.map(str::trim_start).filter(|text| !text.is_empty()) else {
            return false;
        };
        let origin = self.turn_origin.as_deref();
        self.kind.as_deref() == Some("user")
            && self.sidechain != Some(true)
            && self.meta != Some(true)
            && self.compact_summary != Some(true)
            && self.prompt_source.as_deref() != Some("system")
            && !origin.is_some_and(|origin| MACHINE_TURN_ORIGINS.contains(&origin))
            && !text.starts_with("<system-reminder>")
            && (origin == Some("human") || !is_written_by_the_app(text))
    }
}

/// A record of a Claude Code transcript. `None` for a line that is no
/// record.
pub fn claude(line: &str, map: &ProjectMap) -> Option<Observed> {
    claude_with_location(line, map, None)
}

fn observe_claude(record: &ClaudeRecord<'_>, map: &ProjectMap) -> Option<Observed> {
    let content = record.message.as_ref().map(|message| &message.content);
    let mut call: Option<(ToolClass, Named)> = None;
    for block in content.iter().flat_map(|content| &content.calls) {
        let class = claude_class(block.name.as_deref().unwrap_or(""));
        let named = block
            .input
            .map_or(Named::None, |input| map.named_in(input.get()));
        call = Some(match call {
            Some((known, names)) => (stronger(known, class), names.and(named, map.merges())),
            None => (class, named),
        });
    }
    let program = record
        .entrypoint
        .as_deref()
        .map(|entrypoint| entrypoint.starts_with("sdk-"));
    let closed = record.kind.as_deref() == Some("assistant")
        && record.message.as_ref().is_some_and(|message| {
            matches!(
                message.stop_reason.as_deref(),
                Some("end_turn" | "stop_sequence")
            )
        });
    let text = content.and_then(|content| content.text.as_deref());
    let kind = if call.is_some() {
        Kind::Call
    } else if program != Some(true) && record.is_prompt(text) {
        Kind::Prompt
    } else if closed {
        Kind::TurnEnd
    } else {
        Kind::Other
    };
    Some(Observed {
        at: time(record.timestamp.as_deref()),
        facts: Facts {
            folder: folder(map, record.cwd.as_deref()),
            call,
            // The shell of Claude Code runs where the session is.
            shell_dir: Folder::Unsaid,
        },
        kind,
        program,
    })
}

// ---------------------------------------------------------------------- Codex

#[derive(Deserialize)]
struct CodexRecord<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    payload: Option<CodexPayload<'a>>,
}

#[derive(Deserialize)]
struct CodexPayload<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    cwd: Field<'a>,
    #[serde(borrow, default)]
    branch: Field<'a>,
    #[serde(borrow, default)]
    git: Option<&'a RawValue>,
    #[serde(borrow, default)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    role: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    originator: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    source: Option<&'a RawValue>,
    #[serde(borrow, default)]
    content: Option<Content<'a>>,
    /// The input of a call, under the name its kind gives it.
    #[serde(borrow, default)]
    arguments: Option<&'a RawValue>,
    #[serde(borrow, default)]
    input: Option<&'a RawValue>,
    #[serde(borrow, default)]
    action: Option<&'a RawValue>,
}

/// The shells of Codex by the names its versions gave them, and the tool
/// that changes files. A patch sent through the shell is a write.
fn codex_class(kind: &str, name: &str, input: &str) -> ToolClass {
    if name == "apply_patch" || input.contains("*** Begin Patch") {
        return ToolClass::Write;
    }
    let shell = kind == "local_shell_call"
        || matches!(
            name,
            "exec" | "shell" | "shell_command" | "exec_command" | "local_shell" | "unified_exec"
        );
    if shell {
        ToolClass::Shell
    } else {
        ToolClass::Read
    }
}

/// The folder a call of the shell says it runs in. The input of a call is
/// a text that holds an object, or the object itself.
fn codex_shell_dir(map: &ProjectMap, input: &RawValue) -> Folder {
    #[derive(Deserialize)]
    struct Where<'a> {
        #[serde(borrow, default)]
        workdir: Option<Cow<'a, str>>,
        #[serde(borrow, default)]
        working_directory: Option<Cow<'a, str>>,
        #[serde(borrow, default)]
        cwd: Option<Cow<'a, str>>,
    }
    let said = |text: &str| -> Option<Folder> {
        let at: Where = serde_json::from_str(text).ok()?;
        let dir = at.workdir.or(at.working_directory).or(at.cwd)?;
        Some(folder(map, Some(&dir)))
    };
    let raw = input.get();
    let inner = if raw.starts_with('"') {
        serde_json::from_str::<Cow<str>>(raw)
            .ok()
            .and_then(|text| said(&text))
    } else {
        said(raw)
    };
    inner.unwrap_or(Folder::Unsaid)
}

/// Whether the opening record of a rollout names a session a program ran:
/// `codex exec` and `codex review`, and a subagent a session spawned.
fn codex_program(payload: &CodexPayload) -> bool {
    let source = payload.source.map(|source| source.get()).unwrap_or("");
    payload.originator.as_deref() == Some("codex_exec")
        || source == "\"exec\""
        || (source.starts_with('{') && source.contains("\"subagent\""))
}

/// A record of a Codex rollout. `None` for a line that is no record.
pub fn codex(line: &str, map: &ProjectMap) -> Option<Observed> {
    codex_with_location(line, map, None)
}

fn observe_codex(record: &CodexRecord<'_>, map: &ProjectMap) -> Option<Observed> {
    let mut observed = Observed {
        at: time(record.timestamp.as_deref()),
        ..Observed::default()
    };
    let Some(payload) = &record.payload else {
        return Some(observed);
    };
    let kind = payload.kind.as_deref().unwrap_or("");
    match record.kind.as_deref() {
        // Where the session is: said when it opens and when a turn begins.
        Some("session_meta") => {
            observed.facts.folder = folder(map, payload.cwd.as_deref());
            observed.program = Some(codex_program(payload));
        }
        Some("turn_context") => observed.facts.folder = folder(map, payload.cwd.as_deref()),
        Some("event_msg") if matches!(kind, "task_complete" | "turn_aborted") => {
            observed.kind = Kind::TurnEnd;
        }
        Some("response_item") => {
            if matches!(
                kind,
                "function_call" | "custom_tool_call" | "local_shell_call"
            ) {
                let mut named = Named::None;
                let mut class = ToolClass::Read;
                for input in [payload.arguments, payload.input, payload.action]
                    .into_iter()
                    .flatten()
                {
                    let text = input.get();
                    named = named.and(map.named_in(text), map.merges());
                    class = stronger(
                        class,
                        codex_class(kind, payload.name.as_deref().unwrap_or(""), text),
                    );
                    if observed.facts.shell_dir == Folder::Unsaid {
                        observed.facts.shell_dir = codex_shell_dir(map, input);
                    }
                }
                observed.facts.call = Some((class, named));
                observed.kind = Kind::Call;
            } else if kind == "message" && payload.role.as_deref() == Some("user") {
                // What the client puts into the user's role opens with a
                // tag; what the owner types does not.
                let text = payload.content.as_ref().and_then(|c| c.text.as_deref());
                if text
                    .map(str::trim_start)
                    .is_some_and(|text| !text.is_empty() && !text.starts_with('<'))
                {
                    observed.kind = Kind::Prompt;
                }
            }
        }
        _ => {}
    }
    Some(observed)
}

#[cfg(test)]
#[path = "facts_tests.rs"]
mod tests;
