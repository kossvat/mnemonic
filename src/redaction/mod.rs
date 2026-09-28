//! Secret redaction at capture, policy version 1.
//!
//! A deterministic, local pass over text headed for storage. Explicit
//! `<private>` regions and recognized credential shapes are replaced with
//! fixed markers before anything is classified, excerpted, embedded or
//! persisted. Recognition is by documented shape only (no network, no model,
//! no entropy test): a clean result means "no recognized finding", never
//! "no secret". What is recognized and what is not is described in the
//! README, under "Secret redaction".
//!
//! Narrative text is prepared ([`redact_text`], [`prepare_entry`]); fields
//! that identify something (subjects, keys, projects, names) are rejected
//! instead ([`check_identity`]), because masking an identity changes what it
//! points at.

mod assign;
mod detect;
pub mod failure;
mod json;
mod prepare;
mod private;
pub mod state;

#[cfg(test)]
mod bypass_tests;
#[cfg(test)]
mod prepare_tests;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

pub use json::{Limits, inspect_json, parse_strict, redact_json};
#[allow(unused_imports)]
pub use prepare::{PreparedEntry, PreparedEvent, SUMMARY_KEY, Summary};
pub use prepare::{
    STRUCTURAL_KEYS, check_entry, check_event, is_time, prepare_entry, prepare_event, replay_event,
    reprepare_entry,
};

/// Bumped whenever recognition changes what gets redacted.
pub const POLICY_VERSION: u32 = 1;
/// Replaces a recognized credential. Exactly this text counts as redacted.
pub const CREDENTIAL_MARKER: &str = "[REDACTED:credential]";
/// Replaces an explicit `<private>` region, nested regions included.
pub const PRIVATE_MARKER: &str = "[REDACTED:private]";

/// What a redaction was for. The names are stable: they appear in stored
/// summaries and scanner reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    PrivateBlock,
    PrivateKey,
    CredentialAssignment,
    UrlPassword,
    BearerToken,
    ProviderToken,
    Jwt,
}

impl Class {
    pub const ALL: [Class; 7] = [
        Class::PrivateBlock,
        Class::PrivateKey,
        Class::CredentialAssignment,
        Class::UrlPassword,
        Class::BearerToken,
        Class::ProviderToken,
        Class::Jwt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Class::PrivateBlock => "private_block",
            Class::PrivateKey => "private_key",
            Class::CredentialAssignment => "credential_assignment",
            Class::UrlPassword => "url_password",
            Class::BearerToken => "bearer_token",
            Class::ProviderToken => "provider_token",
            Class::Jwt => "jwt",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Class::PrivateBlock => PRIVATE_MARKER,
            _ => CREDENTIAL_MARKER,
        }
    }
}

/// How many regions of each class one pass redacted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts(BTreeMap<Class, u32>);

// `allow(dead_code)`: the accessors serve tests and the scanner, not the bin.
#[allow(dead_code)]
impl Counts {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, class: Class) -> u32 {
        self.0.get(&class).copied().unwrap_or(0)
    }

    pub fn total(&self) -> u32 {
        self.0.values().sum()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Class, u32)> + '_ {
        self.0.iter().map(|(class, n)| (*class, *n))
    }

    pub fn merge(&mut self, other: &Counts) {
        for (class, n) in other.iter() {
            self.add_n(class, n);
        }
    }

    fn add(&mut self, class: Class) {
        self.add_n(class, 1);
    }

    fn add_n(&mut self, class: Class, n: u32) {
        let slot = self.0.entry(class).or_insert(0);
        *slot = slot.saturating_add(n);
    }
}

/// Why something could not be admitted. Carries a fixed code only: never
/// the input, which is exactly what must not reach a log or a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionError {
    /// A field that identifies something holds a recognized credential or
    /// private region; it is refused rather than masked.
    SensitiveContent,
    /// A structure nests deeper than [`Limits::max_depth`].
    TooDeep,
    /// A structure exceeds [`Limits::max_nodes`] or [`Limits::max_text_bytes`].
    TooLarge,
    /// Metadata carries a [`SUMMARY_KEY`] entry that preparation did not
    /// write: an ingress vouching for its own content.
    InvalidSummary,
}

impl RedactionError {
    pub fn code(self) -> &'static str {
        match self {
            RedactionError::SensitiveContent => "SENSITIVE_CONTENT",
            RedactionError::TooDeep => "STRUCTURE_TOO_DEEP",
            RedactionError::TooLarge => "STRUCTURE_TOO_LARGE",
            RedactionError::InvalidSummary => "INVALID_REDACTION_SUMMARY",
        }
    }
}

impl std::fmt::Display for RedactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for RedactionError {}

/// The result of one pass: the prepared value, what was redacted, and
/// whether anything changed (an orphan `</private>` changes text without
/// counting as a finding).
#[derive(Debug, Clone, PartialEq)]
pub struct Redacted<T> {
    pub value: T,
    pub counts: Counts,
    pub changed: bool,
}

/// Remove private regions, then replace every recognized credential with
/// its marker. Unaffected bytes are kept exactly; applying it again to its
/// own output changes nothing.
pub fn redact_text(text: &str) -> Redacted<String> {
    redact_with_context(text, |_| None)
}

/// [`redact_text`] plus one span known from outside the text (a value in a
/// credential-named field), found by `context` on the text after private
/// regions are removed. It merges with the text's own findings before
/// anything is replaced, so no detector loses the context it needs.
fn redact_with_context(
    text: &str,
    context: impl FnOnce(&str) -> Option<(usize, usize)>,
) -> Redacted<String> {
    let mut r = one_pass(text, context);
    // Replacing a span can make its surroundings recognizable: a tag that
    // shrank under its limit, text joined around a removal. Repeat until a
    // pass changes nothing, so the result is clean under [`is_clean`] and
    // preparing it again is a no-op. Input that still changes after the
    // last pass is left dirty, and the durable check refuses it.
    for _ in 1..MAX_PASSES {
        let next = one_pass(&r.value, |_| None);
        if !next.changed {
            break;
        }
        r.counts.merge(&next.counts);
        r.value = next.value;
    }
    r.changed = r.value != text;
    r
}

/// Passes before [`redact_text`] gives up on reaching a fixed point.
const MAX_PASSES: usize = 4;

fn one_pass(text: &str, context: impl FnOnce(&str) -> Option<(usize, usize)>) -> Redacted<String> {
    let (stripped, regions) = private::strip(text);
    let mut counts = Counts::default();
    if regions > 0 {
        counts.add_n(Class::PrivateBlock, regions);
    }
    let mut spans = detect::credential_spans(&stripped);
    if let Some((start, end)) = context(&stripped) {
        spans.push(detect::Span {
            start,
            end,
            class: Class::CredentialAssignment,
        });
        spans = detect::resolve(spans);
    }
    let value = if spans.is_empty() {
        stripped
    } else {
        let mut out = String::with_capacity(stripped.len());
        let mut copied = 0;
        for span in &spans {
            out.push_str(&stripped[copied..span.start]);
            out.push_str(span.class.marker());
            counts.add(span.class);
            copied = span.end;
        }
        out.push_str(&stripped[copied..]);
        out
    };
    let changed = value != text;
    Redacted {
        value,
        counts,
        changed,
    }
}

/// True when [`redact_text`] would leave `text` exactly as it is: a pass
/// that changes nothing is its fixed point.
pub fn is_clean(text: &str) -> bool {
    !one_pass(text, |_| None).changed
}

/// Admit a field that identifies something (a subject, key, project, name,
/// source or request id) only when it holds nothing to redact.
pub fn check_identity(value: &str) -> Result<(), RedactionError> {
    if is_clean(value) {
        Ok(())
    } else {
        Err(RedactionError::SensitiveContent)
    }
}
