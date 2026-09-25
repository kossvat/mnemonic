//! The MCP tools agents use for facts: `memory_fact_set` states a value,
//! `memory_facts` reads the current value and what it replaced.

use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

use super::declare::declare;
use super::rule::Trust;
use super::store::FactWrite;
use crate::embedding::Embedder;
use crate::storage::Storage;

pub fn tool_list() -> Vec<Value> {
    vec![
        json!({
            "name": "memory_fact_set",
            "description": "Record the value of a fact that can change later: a price, commission, discount, budget, deadline, payment terms, a status, an owner. The slot is subject + predicate (+ qualifier, e.g. a plan or region) within a project. A newer different value replaces the current one and keeps the old one in its history, the same value again only reconfirms it, and a value with an older as_of date joins the history without becoming current. Prefer this over memory_save for any value an agent may need to look up as 'current'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "What the fact is about: a product, client, supplier, plan"},
                    "predicate": {"type": "string", "description": "Which fact: price, deadline, commission, payment-terms, status, or any short key"},
                    "value": {"type": "string", "description": "The value as said, e.g. \"$6/month\", \"Net 30\", \"2026-10-15\""},
                    "project": {"type": "string", "description": "Project the fact belongs to; values in different projects never replace each other"},
                    "qualifier": {"type": "string", "description": "A variant within the slot: a plan, region, size"},
                    "as_of": {"type": "string", "description": "When the value took effect (RFC 3339 or YYYY-MM-DD); default now"},
                    "note": {"type": "string", "description": "Why or where from; kept with the memory that records this"},
                    "retract": {"type": "boolean", "description": "The fact no longer has a value (value is then not needed)"},
                    "request_id": {"type": "string", "description": "Makes a retry safe: the same id is applied once"},
                    "expected_revision": {"type": "integer", "description": "Refuse unless the fact is still at this revision"},
                    "agent": {"type": "string", "description": "Who is stating it, e.g. claude-code, codex, hermes"}
                },
                "required": ["subject", "predicate"]
            }
        }),
        json!({
            "name": "memory_facts",
            "description": "The current value of each fact about a subject, the value it replaced and when, and the dated history. Use before quoting a price, deadline or term, so an old value is never taken for the current one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "What the facts are about"},
                    "project": {"type": "string", "description": "Only this project's facts (default: every project)"},
                    "history": {"type": "boolean", "description": "Every earlier value, not just the last few"}
                },
                "required": ["subject"]
            }
        }),
    ]
}

const MAX_NOTE: usize = 8000;

fn text<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

pub fn fact_set(
    params: &Value,
    storage: &Storage,
    embedder: &dyn Embedder,
    threshold: f32,
) -> Result<Value> {
    let Some(subject) = text(params, "subject") else {
        bail!("Missing 'subject'");
    };
    let Some(predicate) = text(params, "predicate") else {
        bail!("Missing 'predicate'");
    };
    let retract = params
        .get("retract")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let value = text(params, "value");
    if !retract && value.is_none() {
        bail!("Missing 'value' (or pass retract: true)");
    }
    if let Some(note) = text(params, "note") {
        ensure!(
            note.chars().count() <= MAX_NOTE,
            "a note is at most {MAX_NOTE} characters"
        );
    }
    let write = FactWrite {
        project: text(params, "project"),
        subject,
        predicate,
        qualifier: text(params, "qualifier"),
        value: if retract { None } else { value },
        as_of: text(params, "as_of"),
        trust: Some(Trust::Declared),
        actor: "mcp",
        agent: text(params, "agent"),
        request_id: text(params, "request_id"),
        expected_revision: params.get("expected_revision").and_then(|v| v.as_i64()),
        ..Default::default()
    };
    let declared = declare(storage, embedder, threshold, &write, text(params, "note"))?;
    let outcome = &declared.outcome;
    Ok(json!({
        "outcome": outcome.outcome,
        "replayed": outcome.replayed,
        "fact": outcome.fact,
        "replaced": outcome.replaced,
        "value_id": outcome.value_id,
        "revision": outcome.fact.revision,
        "memory_id": declared.memory.as_ref().map(|m| &m.id),
        "updates": declared
            .link
            .as_ref()
            .zip(declared.memory.as_ref())
            .map(|(link, memory)| vec![crate::updates::plan::link_json(&memory.id, link)])
            .unwrap_or_default(),
    }))
}

/// How many earlier values the short trail shows.
const SHORT_TRAIL: usize = 3;

pub fn facts(params: &Value, storage: &Storage) -> Result<Value> {
    let Some(subject) = text(params, "subject") else {
        bail!("Missing 'subject'");
    };
    let history = params
        .get("history")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut views = super::store::views(storage, text(params, "project"), subject)?;
    if !history {
        for view in &mut views {
            view.history.truncate(SHORT_TRAIL);
        }
    }
    Ok(json!({"count": views.len(), "facts": views}))
}
