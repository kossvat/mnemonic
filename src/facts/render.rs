//! Facts as one line of text, for the CLI.

use super::store::{Outcome, SlotView, ValueView};

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn day(time: &str) -> &str {
    time.get(..10).unwrap_or(time)
}

fn label(view: &SlotView) -> String {
    let mut label = format!("{} {}", view.subject, view.predicate);
    if !view.qualifier.is_empty() {
        label.push_str(&format!(" ({})", view.qualifier));
    }
    if !view.project.is_empty() {
        label.push_str(&format!(" [{}]", view.project));
    }
    label
}

fn value_text(value: &ValueView) -> &str {
    value.value.as_deref().unwrap_or("(retracted)")
}

/// A slot: its current value, the one before it, and with `history` every
/// value with its dates.
pub fn slot(view: &SlotView, history: bool) -> String {
    let mut line = match &view.current {
        Some(current) => format!(
            "{} = {} (since {})",
            label(view),
            value_text(current),
            day(&current.valid_from)
        ),
        None => format!("{} has no value", label(view)),
    };
    if let Some(before) = view.history.get(1) {
        line.push_str(&format!(", was {}", value_text(before)));
    }
    if view.pending > 0 {
        line.push_str(&format!(", {} pending review", view.pending));
    }
    if history {
        for value in &view.history {
            let until = value.valid_to.as_deref().map(day).unwrap_or("now");
            let source = value
                .source_memory_id
                .as_deref()
                .map(short)
                .unwrap_or_default();
            line.push_str(&format!(
                "\n    {} {} to {}  {} {}  {}",
                value_text(value),
                day(&value.valid_from),
                until,
                value.trust,
                source,
                short(&value.id)
            ));
        }
    }
    line
}

/// What a write did.
pub fn outcome(outcome: &Outcome) -> String {
    let what = if outcome.replayed {
        "already applied".to_owned()
    } else {
        outcome.outcome.replace('_', " ")
    };
    let mut line = format!("{what}: {}", slot(&outcome.fact, false));
    if let Some(replaced) = &outcome.replaced {
        line.push_str(&format!(" (replaced {})", value_text(replaced)));
    }
    line
}
