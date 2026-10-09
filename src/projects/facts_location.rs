//! Optional in-memory location updates from the observation parser's fields.
//! The facts-only API and its copyable result keep their original shape.
use super::{ClaudeRecord, CodexRecord, Observed, ProjectMap, observe_claude, observe_codex};
use crate::plans::location::Location;

pub(crate) fn claude_with_location(
    line: &str,
    map: &ProjectMap,
    location: Option<&mut Location>,
) -> Option<Observed> {
    let record: ClaudeRecord = serde_json::from_str(line).ok()?;
    // Cwd was already a typed fact before locations were observed. Keep
    // rejecting an invalid cwd, while an invalid optional branch cannot
    // discard a prompt, tool call or other valid fact.
    if record.cwd.is_invalid() {
        return None;
    }
    if let Some(location) = location {
        location.claude_fields(&record.cwd, &record.branch);
    }
    observe_claude(&record, map)
}

pub(crate) fn codex_with_location(
    line: &str,
    map: &ProjectMap,
    location: Option<&mut Location>,
) -> Option<Observed> {
    let record: CodexRecord = serde_json::from_str(line).ok()?;
    if let Some(payload) = &record.payload {
        if payload.cwd.is_invalid() {
            return None;
        }
        if let Some(location) = location {
            location.codex_fields(
                record.kind.as_deref().unwrap_or(""),
                &payload.cwd,
                &payload.branch,
                payload.git,
            );
        }
    }
    observe_codex(&record, map)
}
