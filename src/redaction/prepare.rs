//! Memory entries and capture events: prepared once at admission, checked
//! again at every durable write.

use serde_json::{Map, Value, json};

use super::{
    Class, Counts, Limits, POLICY_VERSION, Redacted, RedactionError, check_identity, is_clean,
    redact_json, redact_text,
};
use crate::event::{Event, EventKind, MemoryEntry};

/// The metadata field reserved for the summary of what admission redacted.
pub const SUMMARY_KEY: &str = "redaction";

/// Metadata fields of a memory that identify something: a transcript path,
/// a speaker role, a commit id, a file path, a project and its key, the
/// session a summary is of. Refused, never masked.
pub const STRUCTURAL_KEYS: &[&str] = &[
    "jsonl_path",
    "role",
    "agent",
    "commit_id",
    "path",
    "extension",
    "history",
    "project",
    "project_key",
    "summary_of_session",
];

/// What preparing one record redacted: never the matched text, its length
/// or a hash of it, only fixed classes and counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    changed: bool,
    counts: Counts,
}

#[allow(dead_code)]
impl Summary {
    pub fn changed(&self) -> bool {
        self.changed
    }

    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// `{"policy_version": 1, "changed": true, "counts": {"jwt": 1}}`.
    pub fn to_json(&self) -> Value {
        let counts: Map<String, Value> = self
            .counts
            .iter()
            .map(|(class, n)| (class.as_str().to_string(), json!(n)))
            .collect();
        json!({
            "policy_version": POLICY_VERSION,
            "changed": self.changed,
            "counts": counts,
        })
    }

    /// The summary a stored record carries, if it is exactly what
    /// [`Summary::to_json`] writes for a changed record under this policy.
    /// Anything else under [`SUMMARY_KEY`] came from somewhere else.
    fn from_json(value: &Value) -> Option<Summary> {
        let map = value.as_object()?;
        if map.len() != 3
            || map.get("policy_version").and_then(Value::as_u64) != Some(u64::from(POLICY_VERSION))
            || map.get("changed") != Some(&Value::Bool(true))
        {
            return None;
        }
        let mut counts = Counts::default();
        for (name, n) in map.get("counts")?.as_object()? {
            let class = *Class::ALL.iter().find(|c| c.as_str() == name)?;
            let n = u32::try_from(n.as_u64()?).ok().filter(|n| *n > 0)?;
            counts.add_n(class, n);
        }
        Some(Summary {
            changed: true,
            counts,
        })
    }

    fn absorb<T>(&mut self, r: &Redacted<T>) {
        self.changed |= r.changed;
        self.counts.merge(&r.counts);
    }
}

/// An entry that went through [`prepare_entry`]. Only preparation builds
/// one, so holding it means its text was prepared under this policy.
#[derive(Debug, Clone)]
pub struct PreparedEntry {
    entry: MemoryEntry,
    summary: Summary,
}

impl PreparedEntry {
    pub fn entry(&self) -> &MemoryEntry {
        &self.entry
    }

    pub fn into_entry(self) -> MemoryEntry {
        self.entry
    }

    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Account for a pass the caller ran on the source text before the
    /// entry existed (a tag list redacted before it was split).
    pub fn absorb<T>(&mut self, r: &Redacted<T>) {
        self.summary.absorb(r);
        write_summary(&mut self.entry.metadata, &self.summary);
    }
}

/// An event that went through [`prepare_event`] or [`replay_event`].
#[derive(Debug, Clone)]
pub struct PreparedEvent {
    event: Event,
    summary: Summary,
}

impl PreparedEvent {
    #[allow(dead_code)]
    pub fn event(&self) -> &Event {
        &self.event
    }

    pub fn into_event(self) -> Event {
        self.event
    }

    #[allow(dead_code)]
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Account for a pass the caller ran on the source text before choosing
    /// what the event holds (a whole message, before its excerpt was cut).
    pub fn absorb<T>(&mut self, r: &Redacted<T>) {
        self.summary.absorb(r);
        write_summary(&mut self.event.metadata, &self.summary);
    }
}

/// Prepare an entry arriving from outside: title, content, tags and
/// metadata values are redacted; the id, strings under `structural_keys`
/// and object keys are refused instead. Whatever the caller put under
/// [`SUMMARY_KEY`] is dropped (an ingress cannot vouch for itself), and a
/// fresh summary is written there when anything changed.
pub fn prepare_entry(
    entry: MemoryEntry,
    structural_keys: &[&str],
) -> Result<PreparedEntry, RedactionError> {
    prepare_entry_keeping(entry, structural_keys, false)
}

/// Prepare a classified entry again before it is embedded or written. The
/// classifier builds a title by cutting the content, and a cut can turn a
/// clean text into a credential shape (`password=f(` loses the `(` that
/// made it code). The summary admission wrote is kept and extended.
pub fn reprepare_entry(
    entry: MemoryEntry,
    structural_keys: &[&str],
) -> Result<PreparedEntry, RedactionError> {
    prepare_entry_keeping(entry, structural_keys, true)
}

fn prepare_entry_keeping(
    mut entry: MemoryEntry,
    structural_keys: &[&str],
    keep_summary: bool,
) -> Result<PreparedEntry, RedactionError> {
    check_identity(&entry.id)?;
    let texts = [&mut entry.title, &mut entry.content]
        .into_iter()
        .chain(entry.tags.iter_mut());
    let mut summary = prepare_parts(texts, &mut entry.metadata, structural_keys, keep_summary)?;
    prepare_under(&entry.title, &mut entry.content, &mut summary);
    // Once more, with what reading them together found.
    write_summary(&mut entry.metadata, &summary);
    Ok(PreparedEntry { entry, summary })
}

/// The ways a title and a content are read together: one after the other
/// (what a memory is embedded by, and with a line between them what a
/// sink writes: to the policy a line break is a space), and the content
/// under the title (a title says what the content is).
const READ_AS: [&str; 2] = [" ", ": "];

/// A text that is only a time is a credential under no name: that a token
/// expires on a date is worth keeping.
pub fn is_time(text: &str) -> bool {
    let text = text.trim();
    chrono::DateTime::parse_from_rfc3339(text).is_ok()
        || chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok()
}

/// Whether a title and a content, each clean, are clean as they are read
/// together.
fn reads_clean(title: &str, content: &str) -> bool {
    is_time(content)
        || READ_AS
            .iter()
            .all(|join| is_clean(&format!("{title}{join}{content}")))
}

/// Prepare `content` as it reads after `title`. Each was prepared alone;
/// together they can read as a credential (a title that ends in a name
/// and its separator, a content that begins with the value), and every
/// copy of a memory shows them together.
fn prepare_under(title: &str, content: &mut String, summary: &mut Summary) {
    if is_time(content) {
        return;
    }
    // A content that was changed is read again in every way. What would
    // not settle in these rounds is refused by the check at the write.
    for _ in 0..READ_AS.len() * 2 {
        let Some((head, read)) = READ_AS.iter().find_map(|join| {
            let head = format!("{title}{join}");
            let read = redact_text(&format!("{head}{content}"));
            read.changed.then_some((head, read))
        }) else {
            return;
        };
        summary.absorb(&read);
        // What was found is in the content: a title that is clean alone
        // begins nothing (what opens a key or a private region is not
        // clean alone). Were it otherwise, the content is not kept.
        *content = read
            .value
            .strip_prefix(&head)
            .map_or_else(|| super::CREDENTIAL_MARKER.to_string(), str::to_string);
    }
}

/// Prepare a capture event arriving from a watcher: content and metadata
/// values are redacted; the id, a custom kind, strings under
/// `structural_keys` and object keys are refused instead. A caller-supplied
/// summary is dropped.
pub fn prepare_event(
    event: Event,
    structural_keys: &[&str],
) -> Result<PreparedEvent, RedactionError> {
    prepare_event_keeping(event, structural_keys, false)
}

/// Prepare a queued event again before it is processed. A summary that
/// admission wrote is kept and extended, so a record replayed from the
/// queue still says what was redacted at capture; an invalid one is dropped.
pub fn replay_event(
    event: Event,
    structural_keys: &[&str],
) -> Result<PreparedEvent, RedactionError> {
    prepare_event_keeping(event, structural_keys, true)
}

fn prepare_event_keeping(
    mut event: Event,
    structural_keys: &[&str],
    keep_summary: bool,
) -> Result<PreparedEvent, RedactionError> {
    check_identity(&event.id)?;
    if let EventKind::Custom(kind) = &event.kind {
        check_identity(kind)?;
    }
    let summary = prepare_parts(
        std::iter::once(&mut event.content),
        &mut event.metadata,
        structural_keys,
        keep_summary,
    )?;
    Ok(PreparedEvent { event, summary })
}

/// Redact `texts` and `metadata`; write the summary under [`SUMMARY_KEY`]
/// when anything changed. With `keep_summary`, a valid stored summary is the
/// starting point (markers are not counted again, so it is the only record
/// of what an earlier pass did); otherwise whatever is there is dropped.
fn prepare_parts<'a>(
    texts: impl Iterator<Item = &'a mut String>,
    metadata: &mut Value,
    structural_keys: &[&str],
    keep_summary: bool,
) -> Result<Summary, RedactionError> {
    let mut summary = Summary::default();
    if let Value::Object(map) = metadata
        && let Some(stored) = map.remove(SUMMARY_KEY)
        && keep_summary
        && let Some(kept) = Summary::from_json(&stored)
    {
        summary = kept;
    }
    for text in texts {
        let r = redact_text(text);
        summary.absorb(&r);
        *text = r.value;
    }
    let prepared = redact_json(
        metadata,
        structural_keys,
        Limits::default().for_preparation(),
    )?;
    summary.absorb(&prepared);
    *metadata = prepared.value;
    write_summary(metadata, &summary);
    Ok(summary)
}

fn write_summary(metadata: &mut Value, summary: &Summary) {
    if !summary.changed {
        return;
    }
    match metadata {
        Value::Object(map) => {
            map.insert(SUMMARY_KEY.to_string(), summary.to_json());
        }
        meta @ Value::Null => {
            let mut map = Map::new();
            map.insert(SUMMARY_KEY.to_string(), summary.to_json());
            *meta = Value::Object(map);
        }
        // Non-object metadata has nowhere to hold a summary.
        _ => {}
    }
}

/// The check at a durable write: would `entry` be stored exactly as it is?
/// It refuses, it never rewrites; a caller that skipped preparation gets an
/// error instead of a silently different record.
pub fn check_entry(entry: &MemoryEntry, structural_keys: &[&str]) -> Result<(), RedactionError> {
    check_identity(&entry.id)?;
    let texts = [&entry.title, &entry.content]
        .into_iter()
        .chain(&entry.tags);
    check_parts(texts, &entry.metadata, structural_keys)?;
    // As it is read: the title and the content together.
    if !reads_clean(&entry.title, &entry.content) {
        return Err(RedactionError::SensitiveContent);
    }
    Ok(())
}

/// [`check_entry`] for a capture event headed for the durable queue.
pub fn check_event(event: &Event, structural_keys: &[&str]) -> Result<(), RedactionError> {
    check_identity(&event.id)?;
    if let EventKind::Custom(kind) = &event.kind {
        check_identity(kind)?;
    }
    check_parts(
        std::iter::once(&event.content),
        &event.metadata,
        structural_keys,
    )
}

fn check_parts<'a>(
    texts: impl Iterator<Item = &'a String>,
    metadata: &Value,
    structural_keys: &[&str],
) -> Result<(), RedactionError> {
    if let Some(summary) = metadata.get(SUMMARY_KEY)
        && Summary::from_json(summary).is_none()
    {
        return Err(RedactionError::InvalidSummary);
    }
    for text in texts {
        if !is_clean(text) {
            return Err(RedactionError::SensitiveContent);
        }
    }
    let prepared = redact_json(metadata, structural_keys, Limits::default())?;
    if prepared.changed {
        return Err(RedactionError::SensitiveContent);
    }
    Ok(())
}
