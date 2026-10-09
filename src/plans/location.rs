//! Session locations used only while the daemon is running.
//!
//! Only client metadata can move a session. Text, calls, and tool results
//! cannot supply a location. This type deliberately has no serialization.

use std::borrow::Cow;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

#[derive(Debug, Clone, Default)]
pub struct Location {
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
}

/// A missing field keeps the last value; an explicit null clears it.
#[derive(Default)]
pub(crate) enum Field<'a> {
    #[default]
    Missing,
    Null,
    Text(Cow<'a, str>),
    Invalid,
}

impl Field<'_> {
    pub(crate) fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Field<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text;
        impl<'de> Visitor<'de> for Text {
            type Value = Field<'de>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a metadata string or null")
            }

            fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(Field::Text(Cow::Borrowed(value)))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(Field::Text(Cow::Owned(value.into())))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Field::Null)
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Field::Invalid)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Field::Invalid)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Field::Invalid)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Field::Invalid)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Field::Invalid)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Field::Invalid)
            }
        }
        deserializer.deserialize_any(Text)
    }
}

#[derive(Deserialize)]
struct Claude<'a> {
    #[serde(borrow, default)]
    cwd: Field<'a>,
    #[serde(rename = "gitBranch", borrow, default)]
    branch: Field<'a>,
}

#[derive(Deserialize)]
struct Codex<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    payload: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Payload<'a> {
    #[serde(borrow, default)]
    cwd: Field<'a>,
    #[serde(borrow, default)]
    branch: Field<'a>,
    #[serde(borrow, default)]
    git: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Git<'a> {
    #[serde(borrow, default)]
    branch: Field<'a>,
}

impl Location {
    /// Recover only complete records before a validated checkpoint, with
    /// no retained transcript text and no access to another file.
    pub fn from_prefix(
        reader: &mut (impl Read + Seek),
        offset: u64,
        agent: &str,
    ) -> std::io::Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(reader.take(offset));
        let mut line = Vec::new();
        let mut location = Self::default();
        while reader.read_until(b'\n', &mut line)? > 0 {
            if line.last() == Some(&b'\n')
                && let Ok(text) = std::str::from_utf8(&line)
            {
                location.observe(text, agent);
            }
            line.clear();
        }
        Ok(location)
    }

    /// Read only the metadata fields of one record, borrowing its strings.
    pub fn observe(&mut self, line: &str, agent: &str) {
        if !line.contains("\"cwd\"")
            && !line.contains("\"gitBranch\"")
            && !line.contains("\"branch\"")
        {
            return;
        }
        match agent {
            "claude" => {
                if let Ok(record) = serde_json::from_str::<Claude<'_>>(line) {
                    self.claude_fields(&record.cwd, &record.branch);
                }
            }
            "codex" => {
                let Ok(record) = serde_json::from_str::<Codex<'_>>(line) else {
                    return;
                };
                if !matches!(
                    record.kind.as_deref(),
                    Some("session_meta" | "turn_context")
                ) {
                    return;
                }
                let Some(payload) = record.payload else {
                    return;
                };
                if let Ok(payload) = serde_json::from_str::<Payload<'_>>(payload.get()) {
                    self.codex_fields(
                        record.kind.as_deref().unwrap_or(""),
                        &payload.cwd,
                        &payload.branch,
                        payload.git,
                    );
                }
            }
            _ => {}
        }
    }

    /// Apply fields already decoded by the observation reader. A malformed
    /// optional field invalidates only this location update, not its facts.
    pub(crate) fn claude_fields(&mut self, cwd: &Field<'_>, branch: &Field<'_>) {
        self.apply(cwd, branch);
    }

    pub(crate) fn codex_fields(
        &mut self,
        kind: &str,
        cwd: &Field<'_>,
        branch: &Field<'_>,
        git: Option<&RawValue>,
    ) {
        if !matches!(kind, "session_meta" | "turn_context") {
            return;
        }
        let git = match git {
            Some(raw) if !raw.get().trim_start().starts_with('{') => return,
            Some(raw) => match serde_json::from_str::<Git<'_>>(raw.get()) {
                Ok(git) => git,
                Err(_) => return,
            },
            None => Git {
                branch: Field::Missing,
            },
        };
        if branch.is_invalid() || git.branch.is_invalid() {
            return;
        }
        let branch = if kind == "turn_context" && !matches!(branch, Field::Missing) {
            branch
        } else {
            &git.branch
        };
        self.apply(cwd, branch);
    }

    fn apply(&mut self, cwd: &Field<'_>, branch: &Field<'_>) {
        if cwd.is_invalid() || branch.is_invalid() {
            return;
        }
        match cwd {
            Field::Missing | Field::Invalid => {}
            Field::Null => self.cwd = None,
            Field::Text(value) => {
                let next = (!value.is_empty()).then(|| Path::new(value.as_ref()));
                if self.cwd.as_deref() != next {
                    self.cwd = next.map(Path::to_path_buf);
                }
            }
        }
        match branch {
            Field::Missing | Field::Invalid => {}
            Field::Null => self.branch = None,
            Field::Text(value) => {
                let next = (!value.is_empty()).then_some(value.as_ref());
                if self.branch.as_deref() != next {
                    self.branch = next.map(str::to_owned);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "location_tests.rs"]
mod tests;
