//! What a fact statement may hold. A fact is authoritative: nothing that
//! names it and nothing it states is rewritten. A credential in any of them
//! refuses the whole statement, before the embedder or the store sees it.
//! Only a note or evidence is narrative, and is prepared.
//!
//! The value is judged alone, in the statement as its memory reads, and
//! under every name it is filed or shown under: as given, as the store
//! keys it (`api key` is filed under `api-key`), and as an existing slot
//! spells it. It is judged in both forms it is stored in: as given and as
//! it is compared (`value_norm` folds case and every kind of space). Under a name, a value that is a time is not a credential:
//! that a token expires on a date is a fact. The statement itself is
//! judged with the value it has, so the memory that records an admitted
//! statement shows it exactly as it was stated.

use anyhow::Result;
use rusqlite::Connection;

use super::keys;
use super::store::{FactWrite, Resolved};
use crate::redaction::state::{check_identities, prepare_capped};
use crate::redaction::{RedactionError, check_identity};

/// Evidence kept with a value.
pub const MAX_EVIDENCE: usize = 500;

/// Why a statement was refused: the code, and which part of it, as a fixed
/// phrase that says nothing of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    pub code: RedactionError,
    pub part: &'static str,
}

const IN_A_FIELD: &str = "in what names the fact";
const IN_THE_VALUE: &str = "in the value";
const IN_THE_STATEMENT: &str = "in the statement as it reads";
const UNDER_A_NAME: &str = "the value under one of the fact's names";
const IN_A_RESOLVED_NAME: &str = "in a name as the store resolves it";

fn judged(result: Result<(), RedactionError>, part: &'static str) -> Result<(), Refused> {
    result.map_err(|code| Refused { code, part })
}

/// The statement as its memory reads: the title and the first sentence.
pub fn statement(write: &FactWrite<'_>) -> (String, String) {
    let mut about = format!("{} {}", write.subject.trim(), write.predicate.trim());
    if let Some(qualifier) = write.qualifier.map(str::trim).filter(|q| !q.is_empty()) {
        about.push_str(&format!(" ({qualifier})"));
    }
    match write.value {
        Some(value) => (format!("{about}: {value}"), format!("{about} is {value}.")),
        None => (
            format!("{about}: retracted"),
            format!("{about} no longer has a value."),
        ),
    }
}

/// The value in the forms it is stored in: as given, and as it is
/// compared.
fn stored_forms(value: &str) -> [String; 2] {
    [value.to_owned(), keys::value_norm(value)]
}

/// The value as the policy judges it under a name the statement does not
/// put right before it: none for a retraction, and none for a time.
fn contextual(value: Option<&str>) -> Option<&str> {
    value.filter(|v| keys::parse_time_ms(v).is_err())
}

/// A name must be clean, and the value under it must be, in each form it
/// is stored in.
fn under(name: &str, value: Option<&str>, part: &'static str) -> Result<(), Refused> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    judged(check_identities([name]), part)?;
    let Some(value) = contextual(value) else {
        return Ok(());
    };
    for form in stored_forms(value) {
        judged(check_identity(&format!("{name}: {form}")), UNDER_A_NAME)?;
    }
    Ok(())
}

/// The statement as it was given. Subject, predicate, qualifier and value
/// are judged in the statement, which shows all four side by side (two
/// names that are clean apart can be a credential together); the fields
/// it does not show are judged one by one; then the value under each name.
pub fn check(write: &FactWrite<'_>) -> Result<(), Refused> {
    judged(
        check_identities(
            [
                write.as_of,
                Some(write.actor),
                write.agent,
                write.source_memory_id,
                write.request_id,
            ]
            .into_iter()
            .flatten(),
        ),
        IN_A_FIELD,
    )?;
    if let Some(value) = write.value {
        let forms = stored_forms(value);
        judged(
            check_identities(forms.iter().map(String::as_str)),
            IN_THE_VALUE,
        )?;
    }
    let (title, content) = statement(write);
    judged(
        check_identities([title.as_str(), content.as_str()]),
        IN_THE_STATEMENT,
    )?;
    for name in [
        Some(write.subject),
        Some(write.predicate),
        write.qualifier,
        write.project,
    ]
    .into_iter()
    .flatten()
    {
        under(name, write.value, IN_A_FIELD)?;
    }
    Ok(())
}

/// The statement as the store would file it: the names its fields resolve
/// to (an alias or a project stored before the policy can lead to a name
/// it refuses), the keys of its slot, and the spellings an existing slot
/// is shown under.
pub fn check_resolved(write: &FactWrite<'_>, resolved: &Resolved) -> Result<(), Refused> {
    let names = [
        resolved.scope.as_str(),
        resolved.scope_key.as_str(),
        resolved.subject_key.as_str(),
        resolved.subject_target.as_deref().unwrap_or(""),
        resolved.predicate.as_str(),
        resolved.qualifier_key.as_str(),
    ];
    for name in names
        .into_iter()
        .chain(resolved.labels.iter().map(String::as_str))
    {
        under(name, write.value, IN_A_RESOLVED_NAME)?;
    }
    Ok(())
}

fn refused(refused: Refused) -> anyhow::Error {
    anyhow::Error::new(refused.code).context(format!(
        "fact refused ({}): {}",
        refused.code.code(),
        refused.part
    ))
}

/// [`check`] with the writer's error: a fixed code and which part, nothing
/// of the input.
pub fn admit(write: &FactWrite<'_>) -> Result<()> {
    check(write).map_err(refused)
}

/// [`check_resolved`] against the store as it is, with the writer's error.
/// Reads only.
pub fn admit_resolved(conn: &Connection, write: &FactWrite<'_>) -> Result<Resolved> {
    let resolved = Resolved::of(conn, write)?;
    check_resolved(write, &resolved).map_err(refused)?;
    Ok(resolved)
}

/// Evidence as it is stored: prepared, within its cap.
pub fn evidence(text: &str) -> String {
    prepare_capped(text, MAX_EVIDENCE).value
}

#[cfg(test)]
#[path = "admit_tests.rs"]
mod tests;
