//! What a shared write may hold. Shared text is read as it is by every
//! agent of the project, so nothing in it is rewritten: a request that
//! holds a credential or a private region, in its text or in what names
//! it, is refused whole with a fixed code, before a retry is answered and
//! before any row is written. Every field is stored exactly as it is
//! given, so that is the form it is judged in, alone and as the row reads:
//! the title over the content, and the text under the key and the title
//! that name it (a key `api_key` makes a credential of a content that is
//! one long word).
//!
//! Taking something out of the store is never refused: a row filed under
//! a name the policy refuses, by a binary before the policy, must stay
//! removable. Those paths only keep such a name out of what they say.
//!
//! The policy is the crate's own and is pure: it opens no store, no
//! configuration and no model. It is the only thing the shared service
//! takes from outside its module.

use anyhow::Result;

use super::types::{Observation, SharedRecord};
use crate::redaction::state::{admit_identities, refused};
use crate::redaction::{RedactionError, is_clean, is_time};

const WHAT: &str = "shared write";

/// What names a shared row: a project, a key, a writer, a principal, an
/// actor, a request or an observation id, and the source a text is
/// attributed to.
pub(super) fn identities<'a>(fields: impl IntoIterator<Item = &'a str>) -> Result<()> {
    admit_identities(WHAT, fields)
}

/// The text of a row, alone and as the row reads under `key` (a record
/// has one, an observation has none until it is promoted).
pub(super) fn text(key: Option<&str>, title: &str, content: &str, source: &str) -> Result<()> {
    let under = |name: &str, text: &str| is_time(text) || is_clean(&format!("{name}: {text}"));
    // The form the store itself joins them in, to search: it holds what
    // either of them holds alone. A content that is only a time is a
    // credential beside no title, and the title is then judged alone.
    let row = if is_time(content) {
        is_clean(title)
    } else {
        is_clean(&format!("{title}\n{content}"))
    };
    let clean = row
        && under(title, content)
        && key.is_none_or(|key| under(key, title) && under(key, content));
    if !clean {
        return Err(refused(WHAT, RedactionError::SensitiveContent));
    }
    identities([source])
}

/// A record as it is stored, before it is returned as the answer to a
/// retry: its text, and every name in it that the request did not give
/// (the project and the key are the request's own, judged with it).
pub(super) fn record(record: &SharedRecord) -> Result<()> {
    identities(
        record
            .published_by
            .as_deref()
            .into_iter()
            .chain(record.origin_observation_id.as_deref())
            .chain(record.origin_writer_id.as_deref()),
    )?;
    text(
        Some(&record.key),
        &record.title,
        &record.content,
        &record.source,
    )
}

/// An observation as it is stored, before it is promoted and before it
/// is returned as the answer to a retry: its text, and every name in it
/// that one of the two requests does not give (the project is given by
/// both, and judged with them).
pub(super) fn observation(observation: &Observation) -> Result<()> {
    identities(
        [
            observation.id.as_str(),
            &observation.writer_id,
            &observation.request_id,
        ]
        .into_iter()
        .chain(observation.principal_id.as_deref())
        .chain(observation.promoted_key.as_deref()),
    )?;
    text(
        observation.promoted_key.as_deref(),
        &observation.title,
        &observation.content,
        &observation.source,
    )
}

/// A name as a message may show it: itself, unless the policy refuses it.
pub(super) fn shown(name: &str) -> &str {
    crate::redaction::state::shown(name)
}

/// Whether `error` is this refusal (as opposed to a rejected field, a
/// conflict or a failed store).
pub(super) fn is_refusal(error: &anyhow::Error) -> bool {
    crate::redaction::state::is_refused(error)
}

#[cfg(test)]
#[path = "redaction_tests.rs"]
mod tests;
