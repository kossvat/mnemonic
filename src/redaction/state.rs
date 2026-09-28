//! Admission for state that is not a memory: facts, follow-ups,
//! conclusions, the graph, peers and sessions.
//!
//! A field that identifies something (a name, key, project, subject, id) is
//! refused, never rewritten: a rewritten identity would name something
//! else. It is judged as it was given, before anything normalizes it, since
//! normalization can drop the very syntax that made it a credential, and
//! in the forms it is stored in, since normalization can also give a name
//! a shape it did not have: every identity is judged lowercased too, and a
//! writer that stores a key made from a name judges that key.
//! Narrative is prepared, and prepared again after the writer's own
//! normalization.

use super::{CREDENTIAL_MARKER, Redacted, RedactionError, check_identity, redact_text};

/// Refuse when any field holds something the policy would rewrite, as it
/// was given or lowercased: names are stored and matched without case, and
/// the shapes of the policy are not (an upper-case `SK-...` reads as a
/// name, the same in lower case as a token).
pub fn check_identities<'a>(
    fields: impl IntoIterator<Item = &'a str>,
) -> Result<(), RedactionError> {
    fields.into_iter().try_for_each(|field| {
        check_identity(field)?;
        if field.chars().any(char::is_uppercase) {
            check_identity(&field.to_lowercase())?;
        }
        Ok(())
    })
}

/// The error a writer returns for a refused field: typed, so a caller can
/// tell a refused write from a failed store, and worded with the fixed code
/// only. `what` names the kind of write, never its input.
pub fn refused(what: &'static str, code: RedactionError) -> anyhow::Error {
    anyhow::Error::new(code).context(format!("{what} refused ({})", code.code()))
}

/// Whether `error` is a writer refusing its input (as opposed to the store
/// failing).
pub fn is_refused(error: &anyhow::Error) -> bool {
    error.downcast_ref::<RedactionError>().is_some()
}

/// [`check_identities`] with the writer's error.
pub fn admit_identities<'a>(
    what: &'static str,
    fields: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<()> {
    check_identities(fields).map_err(|code| refused(what, code))
}

/// An id as a message may show it: the id itself, unless it is one the
/// policy refuses (a refusal must not name what it refused).
pub fn shown(id: &str) -> &str {
    if check_identities([id]).is_ok() {
        id
    } else {
        "(an id the redaction policy refuses)"
    }
}

/// Rounds before [`prepare_normalized`] gives up on a text that a cut keeps
/// turning into a credential's shape.
const MAX_ROUNDS: usize = 4;

/// Narrative a writer normalizes before it stores it (cut to a length,
/// collapsed to one line): prepared, normalized, and prepared again until
/// the normalized text is clean, since a cut or a join can leave a
/// credential's shape behind. What comes back is what `normalize` returns
/// for the prepared text.
pub fn prepare_normalized(text: &str, normalize: impl Fn(&str) -> String) -> Redacted<String> {
    let first = redact_text(text);
    let (mut counts, mut changed, mut value) = (first.counts, first.changed, first.value);
    for _ in 0..MAX_ROUNDS {
        let normalized = normalize(&value);
        let again = redact_text(&normalized);
        if !again.changed {
            return Redacted {
                value: normalized,
                counts,
                changed,
            };
        }
        counts.merge(&again.counts);
        changed = true;
        value = again.value;
    }
    // It did not settle: nothing of it is kept.
    Redacted {
        value: normalize(CREDENTIAL_MARKER),
        counts,
        changed: true,
    }
}

/// What is said of an error where a caller, a log or a terminal reads it:
/// the error's own words, prepared like any text, and cut. An error made
/// of fixed sentences and codes reads as it did. One that carries what it
/// was given (a store's words, a parser's, a name that was looked up)
/// does not carry a credential out with it.
pub fn said(error: &anyhow::Error) -> String {
    said_of(&error.to_string())
}

/// [`said`] with what caused the error, for a terminal.
pub fn said_in_full(error: &anyhow::Error) -> String {
    said_of(&format!("{error:#}"))
}

/// [`said`] of words that are not an error's.
pub fn said_of(words: &str) -> String {
    /// More than any message of the crate, and little enough for a line.
    const SAID: usize = 2000;
    prepare_capped(words, SAID).value
}

/// [`prepare_normalized`] for a column with a length cap and nothing else.
pub fn prepare_capped(text: &str, max_chars: usize) -> Redacted<String> {
    prepare_normalized(text, |t| t.chars().take(max_chars).collect())
}
