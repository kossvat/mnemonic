//! Plans read from project files. Locations and parsed copies live only in
//! memory; reading a plan never starts a process or changes a store.
mod cache;
pub mod counts;
mod discovery;
pub mod gate;
pub mod location;
mod providers;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub use discovery::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Registry,
    Checkbox,
    Funnel,
}

/// Optional choices for one project's plan. Relative paths are resolved
/// inside that project's checkout; a configured checkout must be absolute.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub provider: Option<Provider>,
    pub authority: Option<PathBuf>,
    pub checkout: Option<PathBuf>,
    pub receipt: Option<PathBuf>,
    pub gate_required: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: String,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedUnit {
    pub id: String,
    pub title: String,
    pub tasks: Vec<Task>,
    pub counts: BTreeMap<String, u32>,
    pub waiting_on_owner: Vec<String>,
    pub weekly_count: u32,
    pub proposed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Proposed,
    Planned,
    Building,
    Review,
    Blocked,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentTask {
    pub id: String,
    pub title: String,
    pub source: &'static str,
}

/// What a client may see. No field carries a checkout, a branch or a
/// filename; the provider and fixed error identify an unreadable unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    #[serde(skip)]
    pub closed_at: Option<i64>,
    pub id: String,
    pub title: String,
    pub done: u32,
    pub total: u32,
    pub stage: Stage,
    pub stale: bool,
    pub current_task: Option<CurrentTask>,
    pub counts: BTreeMap<String, u32>,
    pub waiting_on_owner: Vec<String>,
    pub weekly_count: u32,
    pub gate: gate::Gate,
    pub provider: Provider,
    /// Other checkouts with different counts from the selected one.
    pub checkouts: usize,
    pub as_of: i64,
    pub tasks: Vec<Task>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub units: Vec<Unit>,
    pub message: Option<&'static str>,
}

/// Only in memory: where a live session works, and the branch's hint of
/// which task it is doing. A missing location does not guess a checkout.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub last_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Input {
    pub main: PathBuf,
    pub config: Config,
    pub checkouts: Vec<Checkout>,
}

fn task_in_branch(branch: &str, id: &str) -> bool {
    !id.is_empty()
        && branch.match_indices(id).any(|(at, _)| {
            let end = at + id.len();
            !branch[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
                && !branch[end..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
        })
}

fn finish(
    parsed: ParsedUnit,
    provider: Provider,
    gate: gate::Gate,
    modified: i64,
    checkouts: &[Checkout],
    gate_required: bool,
    now: DateTime<Utc>,
) -> Unit {
    let total = parsed.tasks.len() as u32;
    let done = parsed
        .tasks
        .iter()
        .filter(|task| task.status == "done")
        .count() as u32;
    let live = !checkouts.is_empty();
    let blocked = parsed.tasks.iter().any(|task| task.status == "blocked")
        || gate.failed_runs >= 3
        || parsed.error.is_some();
    let stage = if blocked {
        Stage::Blocked
    } else if parsed.proposed || (total == 0 && provider != Provider::Funnel) {
        Stage::Proposed
    } else if total > 0 && done == total {
        if gate.status == gate::GateStatus::Passed || !gate_required {
            Stage::Done
        } else {
            Stage::Review
        }
    } else if live || done > 0 {
        Stage::Building
    } else {
        Stage::Planned
    };
    let current = parsed
        .tasks
        .iter()
        .find(|task| task.status == "in_progress")
        .map(|task| (task, "registry"))
        .or_else(|| {
            parsed
                .tasks
                .iter()
                .filter(|task| {
                    checkouts.iter().any(|checkout| {
                        checkout
                            .branch
                            .as_ref()
                            .is_some_and(|branch| task_in_branch(branch, &task.id))
                    })
                })
                .max_by_key(|task| task.id.len())
                .map(|task| (task, "branch"))
        })
        .or_else(|| {
            parsed
                .tasks
                .iter()
                .find(|task| task.status != "done")
                .map(|task| (task, "first_open"))
        });
    let current_task = current.map(|(task, source)| CurrentTask {
        id: task.id.clone(),
        title: task.title.clone(),
        source,
    });
    let as_of = checkouts
        .iter()
        .map(|checkout| checkout.last_at.timestamp())
        .max()
        .unwrap_or(modified)
        .max(modified);
    Unit {
        closed_at: None,
        id: parsed.id,
        title: parsed.title,
        done,
        total,
        stage,
        stale: now.timestamp().saturating_sub(as_of) >= 14 * 86400,
        current_task,
        counts: parsed.counts,
        waiting_on_owner: parsed.waiting_on_owner,
        weekly_count: parsed.weekly_count,
        gate,
        provider,
        checkouts: 0,
        as_of,
        tasks: parsed.tasks,
        error: parsed.error,
    }
}

impl Unit {
    pub fn line(&self) -> crate::live::PlanLine {
        crate::live::PlanLine {
            unit: self.title.clone(),
            done: self.done,
            total: self.total,
            stage: format!("{:?}", self.stage).to_lowercase(),
            task: self.current_task.as_ref().map(|task| task.title.clone()),
            gate: Some(format!("{:?}", self.gate.status).to_lowercase()),
        }
    }
}

#[cfg(test)]
mod tests;
